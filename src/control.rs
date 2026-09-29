//! Starting, stopping and restarting supervisors: shared by the CLI commands,
//! `resurrect` and the MCP server.

use crate::cli::RestartPolicy;
use crate::error::RunaError;
use crate::state::{self, ProcessMetadata, StateManager};
use anyhow::{Context, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Set on a background supervisor process so `runa run` knows it is one.
pub const SUPERVISOR_ENV: &str = "RUNA_SUPERVISOR";
/// The runa.toml a supervisor was started from, recorded for `runa save`.
pub const CONFIG_ENV: &str = "RUNA_SUPERVISOR_CONFIG";

/// How long to watch a new supervisor for an early failure.
const STARTUP_CHECK: Duration = Duration::from_secs(2);
/// Covers the supervisor's 5s SIGTERM grace period plus its log drain.
pub const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub enum StartOutcome {
    /// The supervisor is up and has spawned the command.
    Started { pid: i32 },
    /// The command ran and finished before the startup check ended.
    Exited,
    /// The supervisor gave up, e.g. the command could not be spawned.
    Failed(String),
}

pub enum StopOutcome {
    Signalled { pid: i32 },
    WasDead,
    NotFound,
}

/// The process's metadata if its supervisor is running.
pub fn running_process(state_manager: &StateManager, name: &str) -> Option<ProcessMetadata> {
    state_manager
        .get_process(name)
        .ok()
        .filter(|meta| state::is_runa_supervisor(meta.pid))
}

/// What to start in the background.
pub struct SpawnSpec<'a> {
    pub name: &'a str,
    pub cmd: &'a str,
    pub restart: RestartPolicy,
    /// `KEY=VALUE` pairs passed as `--env` (visible in `ps`).
    pub cli_env: &'a [String],
    pub cwd: Option<&'a Path>,
    /// Applied to the supervisor's environment, which its child inherits,
    /// so these values never appear on a command line.
    pub extra_env: &'a HashMap<String, String>,
    /// The runa.toml this process comes from, if any.
    pub config_file: Option<&'a Path>,
}

/// Start a background supervisor and wait briefly to see whether it comes up.
pub fn spawn_detached(state_manager: &StateManager, spec: &SpawnSpec) -> Result<StartOutcome> {
    let name = spec.name;
    let exe = std::env::current_exe().context("Failed to get current executable path")?;
    let mut command = std::process::Command::new(exe);
    // Use --flag=value so values starting with '-' aren't parsed as flags.
    command
        .arg("run")
        .arg(format!("--name={name}"))
        .arg(format!("--cmd={}", spec.cmd))
        .arg(format!("--restart={}", spec.restart.as_str()));
    for e in spec.cli_env {
        command.arg(format!("--env={e}"));
    }
    if let Some(cwd) = spec.cwd {
        command.current_dir(cwd);
    }
    command.envs(spec.extra_env);
    command.env(SUPERVISOR_ENV, "1");
    match spec.config_file {
        Some(file) => command.env(CONFIG_ENV, file),
        None => command.env_remove(CONFIG_ENV),
    };

    // Redirect to avoid keeping terminal open
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    let mut child = command
        .spawn()
        .context("Failed to spawn background supervisor")?;
    let supervisor_pid = child.id() as i32;

    let deadline = Instant::now() + STARTUP_CHECK;
    loop {
        if let Ok(meta) = state_manager.get_process(name)
            && meta.pid == supervisor_pid
            && meta.child_pid.is_some()
        {
            return Ok(StartOutcome::Started {
                pid: supervisor_pid,
            });
        }

        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(StartOutcome::Exited);
            }
            let reason = state_manager
                .get_process(name)
                .ok()
                .filter(|meta| meta.pid == supervisor_pid)
                .and_then(|meta| meta.last_error)
                .unwrap_or_else(|| format!("supervisor exited with {status}"));
            return Ok(StartOutcome::Failed(reason));
        }

        if Instant::now() >= deadline {
            // Still starting up; assume it's fine rather than block longer.
            return Ok(StartOutcome::Started {
                pid: supervisor_pid,
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Ask a process's supervisor to shut down, or clean up its state if the
/// supervisor is already gone. Does not wait for the shutdown to finish.
pub fn request_stop(state_manager: &StateManager, name: &str) -> Result<StopOutcome> {
    let meta = match state_manager.get_process(name) {
        Ok(meta) => meta,
        Err(RunaError::ProcessNotFound(_)) => return Ok(StopOutcome::NotFound),
        Err(err) => return Err(err).context(format!("Failed to read state for '{name}'")),
    };
    if meta.pid <= 1 || meta.pid == std::process::id() as i32 {
        anyhow::bail!("Invalid PID {} for process '{}'", meta.pid, name);
    }

    if state::is_runa_supervisor(meta.pid) {
        signal::kill(Pid::from_raw(meta.pid), Signal::SIGTERM).context("Failed to send SIGTERM")?;
        Ok(StopOutcome::Signalled { pid: meta.pid })
    } else {
        state_manager
            .remove_process(name)
            .context("Failed to remove process state")?;
        Ok(StopOutcome::WasDead)
    }
}

/// Ask a running process's supervisor to restart its command. Returns the
/// supervisor PID.
pub fn request_restart(state_manager: &StateManager, name: &str) -> Result<i32> {
    let meta = state_manager
        .get_process(name)
        .with_context(|| format!("Process '{name}' not found"))?;
    if !state::is_runa_supervisor(meta.pid) {
        anyhow::bail!("Process '{name}' is not running.");
    }
    signal::kill(Pid::from_raw(meta.pid), Signal::SIGHUP).context("Failed to send SIGHUP")?;
    Ok(meta.pid)
}

/// Wait until none of `pids` is a running supervisor, or `timeout` passes.
/// Returns the PIDs that are still running.
pub fn wait_for_exit(pids: &[i32], timeout: Duration) -> Vec<i32> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining: Vec<i32> = pids
            .iter()
            .copied()
            .filter(|pid| state::is_runa_supervisor(*pid))
            .collect();
        if remaining.is_empty() || Instant::now() >= deadline {
            return remaining;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
