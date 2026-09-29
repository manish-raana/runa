//! `runa mcp`: a Model Context Protocol server over stdio, so AI agents can
//! manage Runa processes through tool calls instead of parsing CLI output.
//!
//! Messages are newline-delimited JSON-RPC 2.0. Stdout carries only protocol
//! messages, so nothing reachable from here may print to it.

use crate::cli::RestartPolicy;
use crate::config;
use crate::control::{self, SpawnSpec, StopOutcome};
use crate::logs;
use crate::project::{self, Report};
use crate::state::StateManager;
use crate::status;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const DEFAULT_LOG_LINES: usize = 100;

const INSTRUCTIONS: &str = "Runa supervises long-running processes such as dev servers and workers. \
Start processes with start_process (or project_up for a runa.toml) instead of running them in a \
shell, check them with list_processes (running state, ports, restarts, last error), and read \
output with get_logs. Stop processes you started when your task is done unless the user wants \
them kept running.";

/// Serve MCP on stdin/stdout until stdin closes.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.context("Failed to read from stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let responses: Vec<Value> = batch.into_iter().filter_map(handle_message).collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            Ok(message) => handle_message(message),
            Err(err) => Some(error_response(
                Value::Null,
                -32700,
                &format!("Parse error: {err}"),
            )),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}").context("Failed to write to stdout")?;
            stdout.flush().context("Failed to write to stdout")?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC message. Returns `None` for notifications and for
/// responses to requests we never send.
fn handle_message(message: Value) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str)?;
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(&params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    // Use the client's version when we support it, else our newest.
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[SUPPORTED_PROTOCOL_VERSIONS.len() - 1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "runa", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn tool_definitions() -> Value {
    let name_only = json!({
        "type": "object",
        "properties": { "name": { "type": "string", "description": "Process name" } },
        "required": ["name"],
    });
    let project_args = json!({
        "type": "object",
        "properties": {
            "file": {
                "type": "string",
                "description": "Path to runa.toml. Defaults to runa.toml in the server's working directory; pass an absolute path to be safe."
            },
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Processes to act on. Defaults to all processes in the file."
            }
        },
    });

    json!([
        {
            "name": "list_processes",
            "description": "List every process Runa tracks, as JSON: name, running, status (running/failed/dead), pid, child_pid, cmd, cwd, ports it listens on, restarts, last_exit_code, last_error, started_at. Use it to check whether a server is up and which port it uses.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "get_logs",
            "description": "Read the last lines of a process's stdout and stderr logs. Returns immediately.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Process name" },
                    "lines": {
                        "type": "integer",
                        "minimum": 1,
                        "description": format!("Lines per stream (default {DEFAULT_LOG_LINES})")
                    }
                },
                "required": ["name"],
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        },
        {
            "name": "start_process",
            "description": "Start a command in the background under Runa supervision and report whether it came up. The command is split with shell-style quoting but not run through a shell: wrap pipes, && or $VARS in sh -c '...'. Fails if a process with this name is already running.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Unique process name, e.g. myapp-web" },
                    "cmd": { "type": "string", "description": "Command to run, e.g. \"npm run dev\"" },
                    "cwd": { "type": "string", "description": "Working directory (absolute, or relative to the server's working directory). Defaults to the server's working directory." },
                    "restart": {
                        "type": "string",
                        "enum": ["always", "on-failure", "never"],
                        "description": "Restart policy (default always)"
                    },
                    "env": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Extra environment variables. Like `runa run -e`, they appear in `ps` output; use a runa.toml env_file for secrets."
                    }
                },
                "required": ["name", "cmd"],
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
        },
        {
            "name": "stop_process",
            "description": "Gracefully stop a process (SIGTERM to its process group, then SIGKILL after 5s) and wait until it has exited.",
            "inputSchema": name_only,
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false },
        },
        {
            "name": "restart_process",
            "description": "Gracefully restart a running process's command, e.g. after changing code. Logs are kept.",
            "inputSchema": name_only,
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false },
        },
        {
            "name": "project_up",
            "description": "Start the processes defined in a runa.toml (skipping ones already running) and report each result.",
            "inputSchema": project_args,
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        },
        {
            "name": "project_down",
            "description": "Stop the processes defined in a runa.toml and wait for them to exit.",
            "inputSchema": project_args,
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false },
        },
    ])
}

/// Run a tool. Unknown tools are a protocol error; failures inside a tool are
/// reported as a result with `isError` so the agent can see them.
fn call_tool(params: &Value) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "Missing tool name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let result = match name {
        "list_processes" => list_processes(),
        "get_logs" => get_logs(&args),
        "start_process" => start_process(&args),
        "stop_process" => stop_process(&args),
        "restart_process" => restart_process(&args),
        "project_up" => project_up(&args),
        "project_down" => project_down(&args),
        _ => return Err((-32602, format!("Unknown tool: {name}"))),
    };

    Ok(match result {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(err) => {
            json!({ "content": [{ "type": "text", "text": format!("{err:#}") }], "isError": true })
        }
    })
}

fn state_manager() -> Result<StateManager> {
    StateManager::new().context("Failed to initialize state manager")
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing required string argument '{key}'"))
}

fn list_processes() -> Result<String> {
    let rows = status::collect_status_rows(&state_manager()?)?;
    serde_json::to_string_pretty(&rows).context("Failed to serialize status")
}

fn get_logs(args: &Value) -> Result<String> {
    let name = str_arg(args, "name")?;
    let lines = match args.get("lines") {
        None | Some(Value::Null) => DEFAULT_LOG_LINES,
        Some(value) => value
            .as_u64()
            .filter(|n| *n > 0)
            .context("'lines' must be a positive integer")? as usize,
    };
    logs::read_logs(&state_manager()?.get_run_dir(), name, Some(lines))
}

fn start_process(args: &Value) -> Result<String> {
    let name = str_arg(args, "name")?;
    let cmd = str_arg(args, "cmd")?;
    let restart = match args.get("restart") {
        None | Some(Value::Null) => RestartPolicy::Always,
        Some(value) => serde_json::from_value(value.clone())
            .context("'restart' must be always, on-failure or never")?,
    };
    let env: Vec<String> = match args.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(s) => Ok(format!("{key}={s}")),
                Value::Number(n) => Ok(format!("{key}={n}")),
                Value::Bool(b) => Ok(format!("{key}={b}")),
                _ => anyhow::bail!("env.{key} must be a string, number or boolean"),
            })
            .collect::<Result<_>>()?,
        Some(_) => anyhow::bail!("'env' must be an object"),
    };
    let cwd = resolve_dir(args.get("cwd").and_then(Value::as_str))?;

    let state_manager = state_manager()?;
    if let Some(existing) = control::running_process(&state_manager, name) {
        anyhow::bail!(
            "Process '{name}' is already running (PID {}). Use restart_process, or stop it first.",
            existing.pid
        );
    }

    let outcome = control::spawn_detached(
        &state_manager,
        &SpawnSpec {
            name,
            cmd,
            restart,
            cli_env: &env,
            cwd: Some(&cwd),
            extra_env: &HashMap::new(),
            config_file: None,
        },
    )?;
    let message = project::describe_start(name, outcome)?;
    Ok(format!("{name}: {message} in {}", cwd.display()))
}

/// An absolute, existing directory: `dir` relative to our working directory,
/// or the working directory itself.
fn resolve_dir(dir: Option<&str>) -> Result<PathBuf> {
    let base = std::env::current_dir().context("Failed to get working directory")?;
    let path = match dir {
        Some(dir) => base.join(dir),
        None => base,
    };
    std::fs::canonicalize(&path)
        .ok()
        .filter(|p| p.is_dir())
        .with_context(|| format!("cwd {} is not a directory", path.display()))
}

fn stop_process(args: &Value) -> Result<String> {
    let name = str_arg(args, "name")?;
    let state_manager = state_manager()?;
    match control::request_stop(&state_manager, name)? {
        StopOutcome::Signalled { pid } => {
            if control::wait_for_exit(&[pid], control::STOP_WAIT_TIMEOUT).is_empty() {
                Ok(format!("{name}: stopped"))
            } else {
                anyhow::bail!("{name}: still running (PID {pid}) after the stop timeout")
            }
        }
        StopOutcome::WasDead => Ok(format!("{name}: was not running (cleaned up its state)")),
        // Idempotent: stopping something that isn't there is not an error.
        StopOutcome::NotFound => Ok(format!("{name}: not running (no such process)")),
    }
}

fn restart_process(args: &Value) -> Result<String> {
    let name = str_arg(args, "name")?;
    let pid = control::request_restart(&state_manager()?, name)?;
    Ok(format!(
        "{name}: restarting (supervisor PID {pid}). Check list_processes or get_logs to confirm it came back up."
    ))
}

fn project_up(args: &Value) -> Result<String> {
    let (file, names) = project_args(args)?;
    let loaded = config::load(file.as_deref())?;
    reports_result(project::up(&state_manager()?, &loaded, &names)?)
}

fn project_down(args: &Value) -> Result<String> {
    let (file, names) = project_args(args)?;
    let loaded = config::load(file.as_deref())?;
    reports_result(project::down(&state_manager()?, &loaded, &names)?)
}

fn project_args(args: &Value) -> Result<(Option<PathBuf>, Vec<String>)> {
    let file = args
        .get("file")
        .and_then(Value::as_str)
        .map(|f| Path::new(f).to_path_buf());
    let names = match args.get("names") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => {
            serde_json::from_value(value.clone()).context("'names' must be an array of strings")?
        }
    };
    Ok((file, names))
}

/// The per-process report as text; an error if any process failed.
fn reports_result(reports: Vec<Report>) -> Result<String> {
    let text = project::format_reports(&reports);
    if project::failed_count(&reports) > 0 {
        anyhow::bail!("{text}");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_protocol_version() {
        let supported = initialize(&json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(supported["protocolVersion"], "2025-03-26");
        let unknown = initialize(&json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(unknown["protocolVersion"], "2025-06-18");
        assert_eq!(unknown["serverInfo"]["name"], "runa");
    }

    #[test]
    fn ignores_notifications_and_rejects_unknown_methods() {
        assert!(
            handle_message(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );
        let response =
            handle_message(json!({ "jsonrpc": "2.0", "id": 7, "method": "nope" })).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn lists_tools_with_schemas() {
        let response =
            handle_message(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 7);
        for tool in tools {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn unknown_tool_is_a_protocol_error_and_bad_args_are_tool_errors() {
        let unknown = call_tool(&json!({ "name": "nope" })).unwrap_err();
        assert_eq!(unknown.0, -32602);

        let bad = call_tool(&json!({ "name": "get_logs", "arguments": {} })).unwrap();
        assert_eq!(bad["isError"], true);
        assert!(bad["content"][0]["text"].as_str().unwrap().contains("name"));
    }
}
