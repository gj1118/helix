//! `helix.process`: programs run in the background, their output handed to a callback.

use crate::error::Result;
use crate::permissions::{require, Permission};
use mlua::prelude::*;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// What a stream of a program keeps: more is read (so the program is not blocked) and
/// dropped.
pub const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// How long a program may run unless the plugin says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A program to run.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSpec {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

/// What came of running a program, as handed to the plugin.
pub async fn run(spec: ProcessSpec) -> serde_json::Value {
    match run_inner(spec).await {
        Ok(value) => value,
        Err(error) => serde_json::json!({
            "success": false,
            "error": error,
            "stdout": "",
            "stderr": "",
            "timed_out": false,
            "truncated": false,
        }),
    }
}

/// Reads all of `stream`, keeping the first `MAX_OUTPUT` bytes; whether there was more.
async fn read_capped(mut stream: impl AsyncRead + Unpin) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_OUTPUT.saturating_sub(kept.len());
                if n > room {
                    truncated = true;
                }
                kept.extend_from_slice(&buffer[..n.min(room)]);
            }
        }
    }
    (kept, truncated)
}

async fn run_inner(spec: ProcessSpec) -> std::result::Result<serde_json::Value, String> {
    let (program, args) = spec
        .argv
        .split_first()
        .ok_or_else(|| "no program given".to_string())?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(if spec.stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // a program that runs past its time (or Helix exiting) is killed
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let input = spec.stdin.clone();
    let write = async move {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            // A program that does not read its input is no error.
            let _ = stdin.write_all(input.as_bytes()).await;
        }
    };
    let finished = tokio::time::timeout(spec.timeout, async {
        let ((), (out, out_truncated), (err, err_truncated), status) = tokio::join!(
            write,
            read_capped(stdout),
            read_capped(stderr),
            child.wait()
        );
        (out, err, out_truncated || err_truncated, status)
    })
    .await;
    match finished {
        Ok((out, err, truncated, status)) => {
            let status = status.map_err(|e| format!("{program} failed: {e}"))?;
            Ok(serde_json::json!({
                "success": status.success(),
                "code": status.code(),
                "stdout": String::from_utf8_lossy(&out),
                "stderr": String::from_utf8_lossy(&err),
                "timed_out": false,
                "truncated": truncated,
            }))
        }
        Err(_) => {
            let _ = child.kill().await;
            Ok(serde_json::json!({
                "success": false,
                "stdout": "",
                "stderr": "",
                "timed_out": true,
                "truncated": false,
                "error": format!("{program} ran longer than {} ms", spec.timeout.as_millis()),
            }))
        }
    }
}

/// The options of `run`: `{ cwd, stdin, env, timeout }`.
fn read_options(argv: Vec<String>, options: Option<&LuaTable>) -> LuaResult<ProcessSpec> {
    let mut spec = ProcessSpec {
        argv,
        cwd: None,
        stdin: None,
        env: Vec::new(),
        timeout: DEFAULT_TIMEOUT,
    };
    if let Some(options) = options {
        spec.cwd = options.raw_get::<Option<String>>("cwd")?.map(PathBuf::from);
        spec.stdin = options.raw_get("stdin")?;
        if let Some(env) = options.raw_get::<Option<LuaTable>>("env")? {
            for pair in env.pairs::<String, String>() {
                spec.env.push(pair?);
            }
            spec.env.sort();
        }
        if let Some(ms) = options.raw_get::<Option<u64>>("timeout")? {
            spec.timeout = Duration::from_millis(ms.max(1));
        }
    }
    Ok(spec)
}

/// Runs `spec` in the background and hands the result to `callback` later.
fn start(lua: &Lua, spec: ProcessSpec, callback: LuaFunction) -> LuaResult<()> {
    let Some((handler, plugin, id)) = super::ui::pending_callback(lua, callback)? else {
        return Err(LuaError::RuntimeError(
            "Programs can only be run with the editor running".into(),
        ));
    };
    tokio::spawn(async move {
        let result = run(spec).await;
        handler.deliver(plugin, id, result);
    });
    Ok(())
}

/// `(argv, callback)` or `(argv, options, callback)`.
fn arguments(rest: LuaMultiValue) -> LuaResult<(Option<LuaTable>, LuaFunction)> {
    let mut values = rest.into_iter();
    match (values.next(), values.next()) {
        (Some(LuaValue::Function(callback)), None) => Ok((None, callback)),
        (Some(LuaValue::Table(options)), Some(LuaValue::Function(callback))) => {
            Ok((Some(options), callback))
        }
        (Some(LuaValue::Nil), Some(LuaValue::Function(callback))) => Ok((None, callback)),
        _ => Err(LuaError::RuntimeError(
            "Expected options (a table, optional) and a callback function".into(),
        )),
    }
}

pub fn register_process_api(lua: &Lua, helix: &LuaTable) -> Result<()> {
    let process = lua.create_table()?;

    // helix.process.run({ program, args... }, { cwd, stdin, env, timeout }, function(result))
    // - Runs a program in the background; `result` is { success, code, stdout, stderr,
    // timed_out, truncated, error }
    let run = lua.create_function(|lua, (argv, rest): (Vec<String>, LuaMultiValue)| {
        require(lua, Permission::Shell, "helix.process.run")?;
        if argv.is_empty() {
            return Err(LuaError::RuntimeError("No program given".into()));
        }
        let (options, callback) = arguments(rest)?;
        let spec = read_options(argv, options.as_ref())?;
        start(lua, spec, callback)
    })?;
    process.set("run", run)?;

    // helix.process.shell(command, options, function(result)) - The same with a command line
    // for the configured shell (`shell` of `[editor]`)
    let shell = lua.create_function(|lua, (line, rest): (String, LuaMultiValue)| {
        require(lua, Permission::Shell, "helix.process.shell")?;
        let (options, callback) = arguments(rest)?;
        let mut argv = crate::lua::with_editor(|editor| Ok(editor.config().shell.clone()))?;
        if argv.is_empty() {
            return Err(LuaError::RuntimeError("No shell configured".into()));
        }
        argv.push(line);
        let spec = read_options(argv, options.as_ref())?;
        start(lua, spec, callback)
    })?;
    process.set("shell", shell)?;

    helix.set("process", process)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn spec(argv: &[&str]) -> ProcessSpec {
        ProcessSpec {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            stdin: None,
            env: Vec::new(),
            timeout: Duration::from_secs(10),
        }
    }

    #[cfg(unix)]
    #[test]
    fn programs_give_their_output_code_and_input() {
        let rt = rt();
        let result = rt.block_on(run(ProcessSpec {
            stdin: Some("from stdin".into()),
            env: vec![("PLUGIN_VAR".into(), "set".into())],
            ..spec(&[
                "sh",
                "-c",
                "cat; echo \" $PLUGIN_VAR\"; echo oops >&2; exit 3",
            ])
        }));
        let (success, code, stdout, stderr): (bool, i64, &str, &str) = (
            result["success"].as_bool().unwrap(),
            result["code"].as_i64().unwrap(),
            result["stdout"].as_str().unwrap(),
            result["stderr"].as_str().unwrap(),
        );
        assert_eq!(
            (success, code, stdout, stderr),
            (false, 3, "from stdin set\n", "oops\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn slow_programs_are_killed_and_big_output_is_cut() {
        let rt = rt();
        let result = rt.block_on(run(ProcessSpec {
            timeout: Duration::from_millis(100),
            ..spec(&["sleep", "10"])
        }));
        let (timed_out, success): (bool, bool) = (
            result["timed_out"].as_bool().unwrap(),
            result["success"].as_bool().unwrap(),
        );
        assert_eq!((timed_out, success), (true, false));

        let result = rt.block_on(run(spec(&[
            "sh",
            "-c",
            "head -c 17000000 /dev/zero | tr '\\0' x",
        ])));
        let stdout = result["stdout"].as_str().unwrap();
        let truncated: bool = result["truncated"].as_bool().unwrap();
        assert_eq!((stdout.len(), truncated), (MAX_OUTPUT, true));
    }

    #[test]
    fn missing_programs_are_reported() {
        let rt = rt();
        let result = rt.block_on(run(spec(&["no-such-program-for-helix"])));
        let (success, error): (bool, &str) = (
            result["success"].as_bool().unwrap(),
            result["error"].as_str().unwrap(),
        );
        assert!(!success);
        assert!(
            error.starts_with("cannot run no-such-program-for-helix"),
            "{error}"
        );
    }
}
