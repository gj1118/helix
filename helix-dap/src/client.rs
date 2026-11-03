use crate::{
    registry::DebugAdapterId,
    requests::{DisconnectArguments, TerminateArguments},
    transport::{Event, Payload, Request, Response, Transport},
    Error, ProgressMap, ProgressState, Result,
};
use helix_core::syntax::config::{DebugAdapterConfig, DebuggerQuirks};
use helix_dap_types::*;

use serde_json::Value;

use anyhow::anyhow;
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader, BufWriter},
    net::TcpStream,
    process::{Child, Command},
    sync::{
        mpsc::{channel, unbounded_channel, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    time,
};

#[derive(Debug)]
pub struct Client {
    id: DebugAdapterId,
    /// Hands the id the registry gives the client to the task that forwards its messages,
    /// see [`Self::set_id`].
    id_tx: Option<oneshot::Sender<DebugAdapterId>>,
    _process: Option<Child>,
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
    connection_type: Option<ConnectionType>,
    starting_request_args: Option<Value>,
    /// The socket address of the debugger, if using TCP transport.
    pub socket: Option<SocketAddr>,
    pub caps: Option<DebuggerCapabilities>,
    // thread_id -> frames
    pub stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
    /// thread_id -> number of frames the adapter reported for the whole stack, when it did.
    pub total_frames: HashMap<ThreadId, usize>,
    /// What the session is called in the editor (the name of the debug template, say).
    pub name: Option<String>,
    /// The session that started this one (`startDebugging`).
    pub parent: Option<DebugAdapterId>,
    /// Sources this session generated (`sourceReference`s), by the path of their buffer in the
    /// editor, see [`adapter_source_path`].
    pub adapter_sources: HashMap<PathBuf, Source>,
    pub thread_states: ThreadStates,
    pub progress: ProgressMap,
    pub thread_id: Option<ThreadId>,
    /// How many times the session stopped. The stack of a stop is fetched in the background
    /// and dropped when the session stopped again in the meantime.
    pub stops: u64,
    /// Currently active frame for the current thread.
    pub active_frame: Option<usize>,
    pub quirks: DebuggerQuirks,
    /// The config which was used to start this debugger.
    pub config: Option<DebugAdapterConfig>,
    /// The latest `setBreakpoints` request sent for each path, see
    /// [`Self::next_breakpoints_request`].
    breakpoint_requests: HashMap<PathBuf, u64>,
}

impl Client {
    // Spawn a process and communicate with it by either TCP or stdio
    // The returned stream includes the Client ID so consumers can differentiate between multiple
    // clients. The client has no id until the registry gives it one (`set_id`); its messages are
    // held back until then.
    pub async fn process(
        transport: &str,
        command: &str,
        args: Vec<&str>,
        port_arg: Option<&str>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        if command.is_empty() {
            return Result::Err(Error::Other(anyhow!("Command not provided")));
        }
        match (transport, port_arg) {
            ("tcp", Some(port_arg)) => Self::tcp_process(command, args, port_arg).await,
            ("stdio", _) => Self::stdio(command, args),
            _ => Result::Err(Error::Other(anyhow!("Incorrect transport {}", transport))),
        }
    }

    pub fn streams(
        rx: Box<dyn AsyncBufRead + Unpin + Send>,
        tx: Box<dyn AsyncWrite + Unpin + Send>,
        err: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        process: Option<Child>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let (server_rx, server_tx) = Transport::start(rx, tx, err, DebugAdapterId::default());
        let (client_tx, client_rx) = unbounded_channel();
        let (id_tx, id_rx) = oneshot::channel();

        let client = Self {
            id: DebugAdapterId::default(),
            id_tx: Some(id_tx),
            _process: process,
            server_tx,
            request_counter: Arc::new(AtomicU64::new(0)),
            caps: None,
            connection_type: None,
            starting_request_args: None,
            socket: None,
            stack_frames: HashMap::new(),
            total_frames: HashMap::new(),
            name: None,
            parent: None,
            adapter_sources: HashMap::new(),
            thread_states: HashMap::new(),
            progress: HashMap::new(),
            thread_id: None,
            stops: 0,
            active_frame: None,
            quirks: DebuggerQuirks::default(),
            config: None,
            breakpoint_requests: HashMap::new(),
        };

        tokio::spawn(Self::recv(id_rx, server_rx, client_tx));

        Ok((client, client_rx))
    }

    pub async fn tcp(
        addr: std::net::SocketAddr,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let stream = TcpStream::connect(addr).await?;
        let (rx, tx) = stream.into_split();
        Self::streams(Box::new(BufReader::new(rx)), Box::new(tx), None, None)
    }

    pub fn stdio(
        cmd: &str,
        args: Vec<&str>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        // Resolve path to the binary
        let cmd = helix_stdx::env::which(cmd)?;

        let process = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // make sure the process is reaped on drop
            .kill_on_drop(true)
            .spawn();

        let mut process = process?;

        // TODO: do we need bufreader/writer here? or do we use async wrappers on unblock?
        let writer = BufWriter::new(process.stdin.take().expect("Failed to open stdin"));
        let reader = BufReader::new(process.stdout.take().expect("Failed to open stdout"));
        let stderr = BufReader::new(process.stderr.take().expect("Failed to open stderr"));

        Self::streams(
            Box::new(reader),
            Box::new(writer),
            Some(Box::new(stderr)),
            Some(process),
        )
    }

    async fn get_port() -> Option<u16> {
        Some(
            tokio::net::TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                0,
            ))
            .await
            .ok()?
            .local_addr()
            .ok()?
            .port(),
        )
    }

    pub fn starting_request_args(&self) -> Option<&Value> {
        self.starting_request_args.as_ref()
    }

    pub async fn tcp_process(
        cmd: &str,
        args: Vec<&str>,
        port_format: &str,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let port = Self::get_port()
            .await
            .ok_or_else(|| Error::Other(anyhow!("No free port for the debug adapter")))?;

        let process = Command::new(cmd)
            .args(args)
            .args(port_format.replace("{}", &port.to_string()).split(' '))
            // silence messages
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Do not kill debug adapter when leaving, it should exit automatically
            .spawn()?;

        // Wait for adapter to become ready for connection
        time::sleep(time::Duration::from_millis(500)).await;
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let stream = TcpStream::connect(socket).await?;

        let (rx, tx) = stream.into_split();
        let mut result = Self::streams(
            Box::new(BufReader::new(rx)),
            Box::new(tx),
            None,
            Some(process),
        );

        // Set the socket address for the client
        if let Ok((client, _)) = &mut result {
            client.socket = Some(socket);
        }

        result
    }

    async fn recv(
        id: oneshot::Receiver<DebugAdapterId>,
        mut server_rx: UnboundedReceiver<Payload>,
        client_tx: UnboundedSender<(DebugAdapterId, Payload)>,
    ) {
        // The client was dropped before it got an id: nobody listens.
        let Ok(id) = id.await else {
            return;
        };
        while let Some(msg) = server_rx.recv().await {
            let msg = match msg {
                Payload::Event(ev) => Payload::Event(ev),
                Payload::Request(req) => Payload::Request(req),
                // Responses go to their requests; the transport drops any other.
                Payload::Response(_) => continue,
            };
            // The editor is gone.
            if client_tx.send((id, msg)).is_err() {
                break;
            }
        }
    }

    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    /// Gives the client the id the registry knows it by. Its messages, which were held back
    /// until now, are tagged with it from here on.
    pub fn set_id(&mut self, id: DebugAdapterId) {
        self.id = id;
        if let Some(id_tx) = self.id_tx.take() {
            let _ = id_tx.send(id);
        }
    }

    /// Numbers a new `setBreakpoints` request for `path`. Requests for one path are answered
    /// in order but the answers are applied in the background: an answer is only applied when
    /// it is for the latest request, see [`Self::is_latest_breakpoints_request`].
    pub fn next_breakpoints_request(&mut self, path: &std::path::Path) -> u64 {
        let request = self
            .breakpoint_requests
            .get(path)
            .map_or(1, |last| last + 1);
        self.breakpoint_requests.insert(path.to_path_buf(), request);
        request
    }

    pub fn is_latest_breakpoints_request(&self, path: &std::path::Path, request: u64) -> bool {
        self.breakpoint_requests.get(path) == Some(&request)
    }

    /// The name of the session shown in the editor.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| self.config.as_ref().map(|config| config.name.as_str()))
            .unwrap_or("debugger")
    }

    /// Whether the active thread of the session is stopped: `"stopped"` or `"running"`.
    pub fn state_label(&self) -> &'static str {
        match self.thread_id {
            Some(thread) => match self.thread_states.get(&thread) {
                Some(state) if state == "running" => "running",
                _ => "stopped",
            },
            None => "running",
        }
    }

    pub fn connection_type(&self) -> Option<ConnectionType> {
        self.connection_type
    }

    fn next_request_id(&self) -> u64 {
        // > The `seq` for the first message sent by a client or debug adapter
        // > is 1, and for each subsequent message is 1 greater than the
        // > previous message sent by that actor
        // <https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage>
        self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    // Internal, called by specific DAP commands when resuming
    pub fn resume_application(&mut self) {
        if let Some(thread_id) = self.thread_id {
            self.thread_states.insert(thread_id, "running".to_string());
            self.stack_frames.remove(&thread_id);
            self.total_frames.remove(&thread_id);
        }
        self.active_frame = None;
        self.thread_id = None;
    }

    /// Execute a RPC request on the debugger.
    pub fn call<R: helix_dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>>
    where
        R::Arguments: serde::Serialize,
    {
        call::<R>(self.server_tx.clone(), self.next_request_id(), arguments)
    }

    /// Something that sends requests to the adapter without borrowing the client, for work
    /// that needs several requests one after the other in the background.
    pub fn requester(&self) -> Requester {
        Requester {
            server_tx: self.server_tx.clone(),
            request_counter: self.request_counter.clone(),
        }
    }

    pub async fn request<R: helix_dap_types::Request>(
        &self,
        params: R::Arguments,
    ) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        // a future that resolves into the response
        let json = self.call::<R>(params).await?;
        let response = serde_json::from_value(json)?;
        Ok(response)
    }

    pub fn reply(
        &self,
        request_seq: u64,
        command: &str,
        result: core::result::Result<Value, Error>,
    ) -> impl Future<Output = Result<()>> {
        let send = self.deferred_reply(request_seq, command);
        async move { send(result) }
    }

    /// Like [`Self::reply`] for a reply that is only known later: the returned function doesn't
    /// borrow the client and sends the reply when it is called.
    pub fn deferred_reply(
        &self,
        request_seq: u64,
        command: &str,
    ) -> impl FnOnce(core::result::Result<Value, Error>) -> Result<()> + Send + 'static {
        let server_tx = self.server_tx.clone();
        let command = command.to_string();
        move |result| {
            let response = match result {
                Ok(result) => Response {
                    request_seq,
                    command,
                    success: true,
                    message: None,
                    body: Some(result),
                },
                Err(error) => Response {
                    request_seq,
                    command,
                    success: false,
                    message: Some(error.to_string()),
                    body: None,
                },
            };
            server_tx
                .send(Payload::Response(response))
                .map_err(|e| Error::Other(e.into()))
        }
    }

    /// An event that doesn't come from an adapter: the editor sends itself these to get the
    /// results of work it does in the background back on its own task.
    pub fn internal_event(event: &str, body: Value) -> Payload {
        Payload::Event(Event {
            event: event.to_owned(),
            body: Some(body),
        })
    }

    pub fn capabilities(&self) -> &DebuggerCapabilities {
        self.caps.as_ref().expect("debugger not yet initialized!")
    }

    pub fn progress_start(&mut self, event: events::ProgressStartBody) -> String {
        let status = ProgressState::new(event.title, event.message, event.percentage);
        let status_line = status.status_line();
        self.progress.insert(event.progress_id, status);
        status_line
    }

    pub fn progress_update(&mut self, event: events::ProgressUpdateBody) -> Option<String> {
        let status = self.progress.get_mut(&event.progress_id)?;
        status.update(event.message, event.percentage);
        Some(status.status_line())
    }

    pub fn progress_end(&mut self, event: events::ProgressEndBody) -> Option<String> {
        let status = self.progress.remove(&event.progress_id)?;
        Some(status.end_status_line(event.message.as_deref()))
    }

    pub async fn initialize(&mut self, adapter_id: String) -> Result<()> {
        let args = requests::InitializeArguments {
            client_id: Some("hx".to_owned()),
            client_name: Some("helix".to_owned()),
            adapter_id,
            locale: Some("en-us".to_owned()),
            lines_start_at_one: Some(true),
            columns_start_at_one: Some(true),
            path_format: Some("path".to_owned()),
            supports_variable_type: Some(true),
            supports_variable_paging: Some(false),
            supports_run_in_terminal_request: Some(true),
            supports_memory_references: Some(false),
            supports_progress_reporting: Some(true),
            supports_invalidated_event: Some(false),
        };

        let response = self.request::<requests::Initialize>(args).await?;
        self.caps = Some(response);

        Ok(())
    }

    pub fn disconnect(
        &mut self,
        args: Option<DisconnectArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Disconnect>(args)
    }

    pub fn terminate(
        &mut self,
        args: Option<TerminateArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Terminate>(args)
    }

    pub fn launch(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Launch);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Launch>(args)
    }

    pub fn attach(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Attach);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Attach>(args)
    }

    pub fn restart(&self) -> impl Future<Output = Result<Value>> {
        let args = if let Some(args) = &self.starting_request_args {
            args.clone()
        } else {
            Value::Null
        };
        self.call::<requests::Restart>(args)
    }

    /// Sends the breakpoints of `file`. The returned future doesn't borrow the client.
    pub fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> impl Future<Output = Result<Option<Vec<Breakpoint>>>> {
        // A buffer of a source the adapter generated is identified by its reference.
        let source = self.adapter_sources.get(&file).cloned().unwrap_or(Source {
            path: Some(file),
            name: None,
            source_reference: None,
            presentation_hint: None,
            origin: None,
            sources: None,
            adapter_data: None,
            checksums: None,
        });
        let args = requests::SetBreakpointsArguments {
            source,
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };

        let call = self.call::<requests::SetBreakpoints>(args);
        async move {
            let response: requests::SetBreakpointsResponse = serde_json::from_value(call.await?)?;
            Ok(response.breakpoints)
        }
    }

    /// The text of a source the adapter generated. The returned future doesn't borrow the client.
    pub fn source_content(&self, source: Source) -> impl Future<Output = Result<String>> {
        let call = self.call::<requests::LoadSource>(requests::LoadSourceArguments {
            source_reference: source.source_reference.unwrap_or_default(),
            source: Some(source),
        });
        async move {
            let response: requests::LoadSourceResponse = serde_json::from_value(call.await?)?;
            Ok(response.content)
        }
    }

    /// Remembers a source the adapter generated and returns the path its buffer gets.
    pub fn register_adapter_source(&mut self, source: &Source) -> PathBuf {
        let path = adapter_source_path(self.id, source);
        self.adapter_sources.insert(path.clone(), source.clone());
        path
    }

    /// Sends `configurationDone`, when the adapter supports it. The returned future doesn't
    /// borrow the client.
    pub fn configuration_done(&self) -> impl Future<Output = Result<()>> {
        let call = self
            .caps
            .as_ref()
            .and_then(|caps| caps.supports_configuration_done_request)
            .unwrap_or(false)
            .then(|| {
                self.call::<requests::ConfigurationDone>(Some(
                    requests::ConfigurationDoneArguments {},
                ))
            });
        async move {
            if let Some(call) = call {
                call.await?;
            }
            Ok(())
        }
    }

    pub fn continue_thread(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::ContinueArguments { thread_id };

        self.call::<requests::Continue>(args)
    }

    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
    ) -> Result<(Vec<StackFrame>, Option<usize>)> {
        let args = requests::StackTraceArguments {
            thread_id,
            start_frame: None,
            levels: None,
            format: None,
        };

        let response = self.request::<requests::StackTrace>(args).await?;
        Ok((response.stack_frames, response.total_frames))
    }

    /// Fetches `levels` frames of the stack of `thread_id` starting at frame `start`. The
    /// returned future doesn't borrow the client.
    pub fn stack_trace_page(
        &self,
        thread_id: ThreadId,
        start: usize,
        levels: usize,
    ) -> impl Future<Output = Result<(Vec<StackFrame>, Option<usize>)>> {
        let call = self.call::<requests::StackTrace>(requests::StackTraceArguments {
            thread_id,
            start_frame: Some(start),
            levels: Some(levels),
            format: None,
        });
        async move {
            let response: requests::StackTraceResponse = serde_json::from_value(call.await?)?;
            Ok((response.stack_frames, response.total_frames))
        }
    }

    pub fn threads(&self) -> impl Future<Output = Result<Value>> {
        self.call::<requests::Threads>(Some(requests::ThreadsArguments {}))
    }

    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let args = requests::ScopesArguments { frame_id };

        let response = self.request::<requests::Scopes>(args).await?;
        Ok(response.scopes)
    }

    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        let args = requests::VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        };

        let response = self.request::<requests::Variables>(args).await?;
        Ok(response.variables)
    }

    /// Like [`Self::scopes`] but the returned future doesn't borrow the client.
    pub fn scopes_future(&self, frame_id: usize) -> impl Future<Output = Result<Vec<Scope>>> {
        let call = self.call::<requests::Scopes>(requests::ScopesArguments { frame_id });
        async move {
            let response: requests::ScopesResponse = serde_json::from_value(call.await?)?;
            Ok(response.scopes)
        }
    }

    /// Fetches (a page of) the variables of `variables_reference`. `filter` is `"indexed"` or
    /// `"named"` for variables with many children, `start` and `count` page through them. The
    /// returned future doesn't borrow the client.
    pub fn variables_page(
        &self,
        variables_reference: usize,
        filter: Option<&str>,
        start: Option<usize>,
        count: Option<usize>,
    ) -> impl Future<Output = Result<Vec<Variable>>> {
        let call = self.call::<requests::Variables>(requests::VariablesArguments {
            variables_reference,
            filter: filter.map(str::to_owned),
            start,
            count,
            format: None,
        });
        async move {
            let response: requests::VariablesResponse = serde_json::from_value(call.await?)?;
            Ok(response.variables)
        }
    }

    /// Evaluate an expression in a frame. The returned future doesn't borrow the client.
    pub fn eval_future(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> impl Future<Output = Result<requests::EvaluateResponse>> {
        let call = self.call::<requests::Evaluate>(requests::EvaluateArguments {
            expression,
            frame_id,
            context: Some("repl".to_owned()),
            format: None,
        });
        async move { Ok(serde_json::from_value(call.await?)?) }
    }

    pub fn step_in(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.call::<requests::StepIn>(args)
    }

    pub fn step_out(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::StepOut>(args)
    }

    pub fn next(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::NextArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::Next>(args)
    }

    pub fn pause(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::PauseArguments { thread_id };

        self.call::<requests::Pause>(args)
    }

    pub async fn eval(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> Result<requests::EvaluateResponse> {
        let args = requests::EvaluateArguments {
            expression,
            frame_id,
            context: None,
            format: None,
        };

        self.request::<requests::Evaluate>(args).await
    }

    pub fn set_exception_breakpoints(
        &self,
        filters: Vec<String>,
    ) -> impl Future<Output = Result<Value>> {
        let args = requests::SetExceptionBreakpointsArguments { filters };

        self.call::<requests::SetExceptionBreakpoints>(args)
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.stack_frames
            .get(&self.thread_id?)?
            .get(self.active_frame?)
    }
}

/// Sends requests to an adapter; see [`Client::requester`].
#[derive(Debug, Clone)]
pub struct Requester {
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
}

impl Requester {
    pub fn call<R: helix_dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>>
    where
        R::Arguments: serde::Serialize,
    {
        let id = self.request_counter.fetch_add(1, Ordering::Relaxed) + 1;
        call::<R>(self.server_tx.clone(), id, arguments)
    }

    /// Like [`Client::stack_trace_page`].
    pub fn stack_trace_page(
        &self,
        thread_id: ThreadId,
        start: usize,
        levels: usize,
    ) -> impl Future<Output = Result<(Vec<StackFrame>, Option<usize>)>> {
        let call = self.call::<requests::StackTrace>(requests::StackTraceArguments {
            thread_id,
            start_frame: Some(start),
            levels: Some(levels),
            format: None,
        });
        async move {
            let response: requests::StackTraceResponse = serde_json::from_value(call.await?)?;
            Ok((response.stack_frames, response.total_frames))
        }
    }

    pub fn threads(&self) -> impl Future<Output = Result<Vec<Thread>>> {
        let call = self.call::<requests::Threads>(Some(requests::ThreadsArguments {}));
        async move {
            let response: requests::ThreadsResponse = serde_json::from_value(call.await?)?;
            Ok(response.threads)
        }
    }

    pub fn scopes(&self, frame_id: usize) -> impl Future<Output = Result<Vec<Scope>>> {
        let call = self.call::<requests::Scopes>(requests::ScopesArguments { frame_id });
        async move {
            let response: requests::ScopesResponse = serde_json::from_value(call.await?)?;
            Ok(response.scopes)
        }
    }

    pub fn variables(
        &self,
        variables_reference: usize,
    ) -> impl Future<Output = Result<Vec<Variable>>> {
        let call = self.call::<requests::Variables>(requests::VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        });
        async move {
            let response: requests::VariablesResponse = serde_json::from_value(call.await?)?;
            Ok(response.variables)
        }
    }
}

/// Sends request `id` to the adapter and returns a future that waits for its response.
///
/// The request is sent right away, not when the future is first polled: requests go out in the
/// order they are made (the breakpoints before `configurationDone`, say), wherever and whenever
/// their responses are awaited.
fn call<R: helix_dap_types::Request>(
    server_tx: UnboundedSender<Payload>,
    id: u64,
    arguments: R::Arguments,
) -> impl Future<Output = Result<Value>>
where
    R::Arguments: serde::Serialize,
{
    let sent = serde_json::to_value(arguments)
        .map_err(Error::from)
        .and_then(|arguments| {
            let (callback_tx, callback_rx) = channel(1);
            let req = Request {
                back_ch: Some(callback_tx),
                seq: id,
                command: R::COMMAND.to_string(),
                arguments: Some(arguments),
            };
            server_tx
                .send(Payload::Request(req))
                .map_err(|e| Error::Other(e.into()))?;
            Ok(callback_rx)
        });
    async move {
        use std::time::Duration;
        use tokio::time::timeout;

        let mut callback_rx = sent?;

        // TODO: specifiable timeout, delay other calls until initialize success
        let response = timeout(Duration::from_secs(20), callback_rx.recv())
            .await
            .map_err(|_| Error::Timeout(id))? // return Timeout
            .ok_or(Error::StreamClosed)??;

        if !response.success {
            let message = response
                .message
                .clone()
                .unwrap_or_else(|| "DAP request failed".to_string());
            return Err(Error::Other(anyhow!(message)));
        }

        Ok(response.body.unwrap_or_default())
    }
}

/// The directory the directories of all sessions' generated sources are in.
pub fn adapter_source_dir_root() -> PathBuf {
    std::env::temp_dir().join("helix-dap-sources")
}

/// The directory buffers of the sources a session generated live in. Nothing is ever written
/// there: the paths only identify the buffers.
pub fn adapter_source_dir(session: DebugAdapterId) -> PathBuf {
    adapter_source_dir_root().join(session.to_string())
}

/// The path of the buffer for a source an adapter generated.
pub fn adapter_source_path(session: DebugAdapterId, source: &Source) -> PathBuf {
    let name = source
        .name
        .clone()
        .unwrap_or_else(|| format!("source-{}", source.source_reference.unwrap_or_default()));
    // `name` comes from the adapter: keep it a single path component.
    let name: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = if name.is_empty() || name == "." || name == ".." {
        format!("source-{}", source.source_reference.unwrap_or_default())
    } else {
        name
    };
    // Adapters reuse names ("<eval>", "[anonymous]") for different sources: the reference tells
    // them apart. It is a directory of its own so that the file name (and with it the language
    // detected from the extension) stays the source's name.
    adapter_source_dir(session)
        .join(format!(
            "ref-{}",
            source.source_reference.unwrap_or_default()
        ))
        .join(name)
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// A client talking to an in-memory adapter, and the adapter's end of the connection.
    fn client() -> (
        Client,
        UnboundedReceiver<(DebugAdapterId, Payload)>,
        BufReader<DuplexStream>,
        DuplexStream,
    ) {
        let (client_out, adapter_in) = tokio::io::duplex(64 * 1024);
        let (adapter_out, client_in) = tokio::io::duplex(64 * 1024);
        let (client, receiver) = Client::streams(
            Box::new(BufReader::new(client_in)),
            Box::new(client_out),
            None,
            None,
        )
        .unwrap();
        (client, receiver, BufReader::new(adapter_in), adapter_out)
    }

    /// The next message the client sent.
    async fn read_message(adapter_in: &mut BufReader<DuplexStream>) -> Value {
        let mut length = None;
        loop {
            let mut header = String::new();
            adapter_in.read_line(&mut header).await.unwrap();
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length: ") {
                length = Some(value.parse::<usize>().unwrap());
            }
        }
        let mut content = vec![0; length.unwrap()];
        adapter_in.read_exact(&mut content).await.unwrap();
        serde_json::from_slice(&content).unwrap()
    }

    async fn write_message(adapter_out: &mut DuplexStream, message: Value) {
        let content = message.to_string();
        let framed = format!("Content-Length: {}\r\n\r\n{content}", content.len());
        adapter_out.write_all(framed.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn requests_go_out_in_the_order_they_are_made() {
        let (mut client, _receiver, mut adapter_in, _adapter_out) = client();
        client.caps = Some(
            serde_json::from_value(serde_json::json!({
                "supportsConfigurationDoneRequest": true
            }))
            .unwrap(),
        );
        // Neither future is ever polled: making the request sends it.
        let breakpoints = client.set_breakpoints(PathBuf::from("/a.py"), Vec::new());
        let done = client.configuration_done();

        assert_eq!(
            read_message(&mut adapter_in).await["command"],
            "setBreakpoints"
        );
        assert_eq!(
            read_message(&mut adapter_in).await["command"],
            "configurationDone"
        );
        drop((breakpoints, done));
    }

    #[tokio::test]
    async fn messages_wait_for_the_id_and_a_stray_response_does_not_stop_them() {
        let (mut client, mut receiver, _adapter_in, mut adapter_out) = client();
        // A response nobody asked for, then an event.
        write_message(
            &mut adapter_out,
            serde_json::json!({
                "seq": 1, "type": "response", "request_seq": 99, "success": true,
                "command": "threads"
            }),
        )
        .await;
        write_message(
            &mut adapter_out,
            serde_json::json!({"seq": 2, "type": "event", "event": "initialized"}),
        )
        .await;

        // Nothing comes out before the client has its id.
        let early = time::timeout(time::Duration::from_millis(50), receiver.recv()).await;
        assert!(early.is_err());

        let mut ids = slotmap::SlotMap::<DebugAdapterId, ()>::with_key();
        let id = ids.insert(());
        client.set_id(id);
        let (from, payload) = time::timeout(time::Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(from, id);
        assert!(
            matches!(&payload, Payload::Event(event) if event.event == "initialized"),
            "{payload:?}"
        );
    }
}

#[cfg(test)]
mod adapter_source_path_tests {
    use super::*;

    fn source(name: Option<&str>, reference: Option<usize>) -> Source {
        Source {
            name: name.map(str::to_owned),
            path: None,
            source_reference: reference,
            presentation_hint: None,
            origin: None,
            sources: None,
            adapter_data: None,
            checksums: None,
        }
    }

    #[test]
    fn sources_with_the_same_name_get_different_paths() {
        let session = DebugAdapterId::default();
        let first = adapter_source_path(session, &source(Some("<eval>"), Some(1)));
        let second = adapter_source_path(session, &source(Some("<eval>"), Some(2)));
        assert_ne!(first, second);
        assert_eq!(first.file_name().unwrap(), "<eval>");
        assert!(first.starts_with(adapter_source_dir(session)));
    }

    #[test]
    fn names_stay_one_path_component() {
        let session = DebugAdapterId::default();
        let path = adapter_source_path(session, &source(Some("a/b:c"), Some(3)));
        assert_eq!(path.file_name().unwrap(), "a_b_c");
        let path = adapter_source_path(session, &source(Some(".."), Some(4)));
        assert_eq!(path.file_name().unwrap(), "source-4");
    }
}
