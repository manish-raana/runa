//! Starting and stopping supervisors from the CLI: shared by `run --detach`,
//! `stop`, `up` and `down`.

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

/// Start a background supervisor for `cmd` and wait briefly to see whether it
/// comes up. `cwd` and `extra_env` are applied to the supervisor process, which
/// its child inherits, so their values never appear on a command line.
pub fn spawn_detached(
    state_manager: &StateManager,
    name: &str,
    cmd: &str,
    restart: RestartPolicy,
    cli_env: &[String],
    cwd: Option<&Path>,
    extra_env: &HashMap<String, String>,
) -> Result<StartOutcome> {
    let exe = std::env::current_exe().context("Failed to get current executable path")?;
    let mut command = std::process::Command::new(exe);
    // Use --flag=value so values starting with '-' aren't parsed as flags.
    command
        .arg("run")
        .arg(format!("--name={name}"))
        .arg(format!("--cmd={cmd}"))
        .arg(format!("--restart={}", restart.as_str()))
        .arg("--internal-supervisor");
    for e in cli_env {
        command.arg(format!("--env={e}"));
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.envs(extra_env);

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
        signal::kill(Pid::from_raw(meta.pid), Signal::SIGTERM)
            .context("Failed to send SIGTERM")?;
        Ok(StopOutcome::Signalled { pid: meta.pid })
    } else {
        state_manager
            .remove_process(name)
            .context("Failed to remove process state")?;
        Ok(StopOutcome::WasDead)
    }
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
