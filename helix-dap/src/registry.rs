use crate::{Client, Payload, Result, StackFrame};
use futures_executor::block_on;
use futures_util::stream::SelectAll;
use helix_core::syntax::config::DebugAdapterConfig;
use slotmap::SlotMap;
use std::fmt;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The resgistry is a struct that manages and owns multiple debugger clients
/// This holds the responsibility of managing the lifecycle of each client
/// plus showing the heirarcihical nature betweeen them
pub struct Registry {
    inner: SlotMap<DebugAdapterId, Client>,
    /// The active debugger client
    ///
    /// TODO: You can have multiple active debuggers, so the concept of a single active debugger
    /// may need to be changed
    current_client_id: Option<DebugAdapterId>,
    /// A stream of incoming messages from all debuggers
    pub incoming: SelectAll<UnboundedReceiverStream<(DebugAdapterId, Payload)>>,
}

impl Registry {
    /// Creates a new DebuggerService instance
    pub fn new() -> Self {
        Self {
            inner: SlotMap::with_key(),
            current_client_id: None,
            incoming: SelectAll::new(),
        }
    }

    pub fn start_client(
        &mut self,
        socket: Option<std::net::SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> Result<DebugAdapterId> {
        self.inner.try_insert_with_key(|id| {
            let result = match socket {
                Some(socket) => block_on(Client::tcp(socket, id)),
                None => block_on(Client::process(
                    &config.transport,
                    &config.command,
                    config.args.iter().map(|arg| arg.as_str()).collect(),
                    config.port_arg.as_deref(),
                    id,
                )),
            };

            let (mut client, receiver) = result?;
            self.incoming.push(UnboundedReceiverStream::new(receiver));

            client.config = Some(config.clone());
            block_on(client.initialize(config.name.clone()))?;
            client.quirks = config.quirks.clone();

            Ok(client)
        })
    }

    /// Removes a session. When it was the selected one, another session is selected: one that
    /// is stopped if there is one.
    pub fn remove_client(&mut self, id: DebugAdapterId) {
        self.inner.remove(id);
        if self.current_client_id == Some(id) || self.get_active_client().is_none() {
            self.current_client_id = self
                .inner
                .iter()
                .find(|(_, client)| client.state_label() == "stopped")
                .or_else(|| self.inner.iter().next())
                .map(|(id, _)| id);
        }
    }

    /// All sessions, parents before their children.
    pub fn sessions(&self) -> Vec<(DebugAdapterId, &Client)> {
        let mut sessions: Vec<_> = self.inner.iter().collect();
        // A child sorts right after its parent; sessions are otherwise kept in the order of the
        // (stable) slot map.
        let mut ordered = Vec::with_capacity(sessions.len());
        let mut roots: Vec<_> = sessions
            .iter()
            .filter(|(_, client)| {
                client
                    .parent
                    .is_none_or(|parent| !self.inner.contains_key(parent))
            })
            .cloned()
            .collect();
        sessions.retain(|entry| !roots.iter().any(|root| root.0 == entry.0));
        roots.reverse();
        while let Some(next) = roots.pop() {
            ordered.push(next);
            let (children, rest): (Vec<_>, Vec<_>) = sessions
                .drain(..)
                .partition(|(_, client)| client.parent == Some(next.0));
            sessions = rest;
            roots.extend(children.into_iter().rev());
        }
        ordered.extend(sessions);
        ordered
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn active_id(&self) -> Option<DebugAdapterId> {
        self.current_client_id
            .filter(|id| self.inner.contains_key(*id))
    }

    pub fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.inner.get(id)
    }

    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.inner.get_mut(id)
    }

    pub fn get_active_client(&self) -> Option<&Client> {
        self.current_client_id.and_then(|id| self.get_client(id))
    }

    pub fn get_active_client_mut(&mut self) -> Option<&mut Client> {
        self.current_client_id
            .and_then(|id| self.get_client_mut(id))
    }

    pub fn set_active_client(&mut self, id: DebugAdapterId) {
        if self.get_client(id).is_some() {
            self.current_client_id = Some(id);
        } else {
            self.current_client_id = None;
        }
    }

    pub fn unset_active_client(&mut self) {
        self.current_client_id = None;
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_client()
            .and_then(|debugger| debugger.current_stack_frame())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

slotmap::new_key_type! {
    pub struct DebugAdapterId;
}

impl fmt::Display for DebugAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
