use crate::cli::RestartPolicy;
use crate::error::Result;
use crate::logs::LogHandler;
use crate::process::ProcessManager;
use crate::state::{ProcessMetadata, ProcessStatus, StateManager};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Child;
use tokio::signal;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub struct Supervisor {
    name: String,
    cmd: String,
    restart_policy: RestartPolicy,
    env_vars: HashMap<String, String>,
    state_manager: StateManager,
}

const INITIAL_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
const STABLE_RUN_DURATION: Duration = Duration::from_secs(30);
const LOG_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

impl Supervisor {
    pub fn new(
        name: String,
        cmd: String,
        restart_policy: RestartPolicy,
        env_vars: HashMap<String, String>,
    ) -> Result<Self> {
        let state_manager = StateManager::new()?;
        Ok(Self {
            name,
            cmd,
            restart_policy,
            env_vars,
            state_manager,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let supervisor_pid = std::process::id() as i32;
        info!(name = %self.name, pid = supervisor_pid, "Supervisor starting...");

        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
        let mut sighup = signal::unix::signal(signal::unix::SignalKind::hangup())?;
        // A persistent SIGINT stream (rather than a fresh ctrl_c() per select)
        // so an interrupt that arrives during the restart delay is not lost.
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt())?;

        let created_at = unix_now();
        let cwd = std::env::current_dir()
            .ok()
            .map(|dir| dir.display().to_string());
        let mut restart_count = 0;
        let mut restart_delay = INITIAL_RESTART_DELAY;
        let mut last_exit_code = None;
        let mut last_exit_at = None;
        let mut logs_cleared = false;

        loop {
            // Register process state
            let mut meta = ProcessMetadata {
                schema_version: 1,
                name: self.name.clone(),
                pid: supervisor_pid,
                child_pid: None,
                cmd: self.cmd.clone(),
                status: ProcessStatus::Running,
                created_at,
                restart_count,
                last_exit_code,
                last_exit_at,
                cwd: cwd.clone(),
                last_error: None,
            };
            self.state_manager.save_process(&meta)?;

            // Clear logs when the supervisor starts (Truncate policy). This must
            // happen after save_process has claimed the name, otherwise a
            // rejected duplicate start would wipe the running process's logs.
            if !logs_cleared {
                LogHandler::clear_logs(&self.name, self.state_manager.get_run_dir());
                logs_cleared = true;
            }

            // Spawn process
            info!(name = %self.name, cmd = %self.cmd, "Spawning child process...");
            let mut child = match ProcessManager::spawn(&self.cmd, &self.env_vars) {
                Ok(c) => c,
                Err(e) => {
                    error!(name = %self.name, error = %e, "Failed to spawn process");
                    self.record_failure(&e.to_string())?;
                    return Err(e);
                }
            };

            // Update child pid in state
            if let Some(child_pid) = child.id() {
                meta.child_pid = Some(child_pid as i32);
                let _ = self.state_manager.save_process(&meta);
            }

            // Setup logging
            let log_dir = self.state_manager.get_run_dir();
            let mut log_tasks = Vec::new();
            if let Some(stdout) = child.stdout.take() {
                log_tasks.push(LogHandler::stream_stdout(
                    stdout,
                    self.name.clone(),
                    log_dir.clone(),
                ));
            }
            if let Some(stderr) = child.stderr.take() {
                log_tasks.push(LogHandler::stream_stderr(stderr, self.name.clone(), log_dir));
            }
            let started_at = Instant::now();

            // Wait for child exit or shutdown signal
            let status = tokio::select! {
                exit_status = child.wait() => {
                    exit_status?
                }
                _ = sigint.recv() => {
                    info!(name = %self.name, "Received Ctrl+C, shutting down gracefully...");
                    self.graceful_shutdown(child).await;
                    drain_log_tasks(log_tasks).await;
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sigterm.recv() => {
                    info!(name = %self.name, "Received SIGTERM, shutting down gracefully...");
                    self.graceful_shutdown(child).await;
                    drain_log_tasks(log_tasks).await;
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sighup.recv() => {
                    info!(name = %self.name, "Received SIGHUP, restarting process...");
                    self.graceful_shutdown(child).await;
                    drain_log_tasks(log_tasks).await;
                    // Loop will continue and restart process
                    continue;
                }
            };
            drain_log_tasks(log_tasks).await;

            // Process exited
            if status.success() {
                info!(name = %self.name, code = status.code(), "Process exited successfully");
            } else {
                warn!(name = %self.name, code = status.code(), "Process exited with error");
            }
            last_exit_code = status.code();
            last_exit_at = Some(unix_now());
            self.update_exit_metadata(last_exit_code, last_exit_at.unwrap())?;

            let should_restart = match self.restart_policy {
                RestartPolicy::Always => true,
                RestartPolicy::OnFailure => !status.success(),
                RestartPolicy::Never => false,
            };
            if !should_restart {
                info!(name = %self.name, policy = ?self.restart_policy, "Process finished, not restarting.");
                self.update_status(ProcessStatus::Stopped)?;
                break;
            }

            // A process that ran stably is not crash-looping; reset the backoff.
            if started_at.elapsed() >= STABLE_RUN_DURATION {
                restart_delay = INITIAL_RESTART_DELAY;
            }

            restart_count += 1;
            info!(name = %self.name, policy = ?self.restart_policy, delay_ms = restart_delay.as_millis(), "Restarting...");
            self.update_status(ProcessStatus::Running)?;

            // Stay responsive to signals while waiting to restart.
            tokio::select! {
                _ = sleep(restart_delay) => {}
                _ = sigint.recv() => {
                    info!(name = %self.name, "Received Ctrl+C during restart delay, shutting down...");
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sigterm.recv() => {
                    info!(name = %self.name, "Received SIGTERM during restart delay, shutting down...");
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sighup.recv() => {
                    info!(name = %self.name, "Received SIGHUP, restarting immediately...");
                }
            }
            restart_delay = next_restart_delay(restart_delay);
        }

        self.cleanup()?;
        Ok(())
    }

    async fn graceful_shutdown(&self, mut child: Child) {
        if let Some(pid) = child.id() {
            let pgid = pid as i32;
            if pgid <= 1 {
                warn!(name = %self.name, child_pid = pid, "Invalid child PID, skipping group kill");
                return;
            }

            info!(name = %self.name, child_pid = pid, group_id = pgid, "Sending SIGTERM to process group...");

            // Send SIGTERM to the process group (negative PID)
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-pgid),
                nix::sys::signal::Signal::SIGTERM,
            );

            let timeout = sleep(Duration::from_secs(5));
            tokio::select! {
                _ = child.wait() => {
                    info!(name = %self.name, "Child process exited.");
                }
                _ = timeout => {
                    warn!(name = %self.name, "Timeout reached, sending SIGKILL to process group...");
                    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), nix::sys::signal::Signal::SIGKILL);
                    // Reap the child so it doesn't linger (e.g. still holding its
                    // port) when we restart.
                    let _ = child.wait().await;
                }
            }
        }
    }

    fn update_status(&self, status: ProcessStatus) -> Result<()> {
        if let Ok(mut meta) = self.state_manager.get_process(&self.name) {
            meta.status = status;
            self.state_manager.save_process(&meta)?;
        }
        Ok(())
    }

    fn record_failure(&self, reason: &str) -> Result<()> {
        if let Ok(mut meta) = self.state_manager.get_process(&self.name) {
            meta.status = ProcessStatus::Failed;
            meta.last_error = Some(reason.to_string());
            self.state_manager.save_process(&meta)?;
        }
        Ok(())
    }

    fn update_exit_metadata(&self, exit_code: Option<i32>, exit_at: u64) -> Result<()> {
        if let Ok(mut meta) = self.state_manager.get_process(&self.name) {
            meta.last_exit_code = exit_code;
            meta.last_exit_at = Some(exit_at);
            self.state_manager.save_process(&meta)?;
        }
        Ok(())
    }

    fn cleanup(&self) -> Result<()> {
        info!(name = %self.name, "Cleaning up process state...");
        self.state_manager.remove_process(&self.name)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn next_restart_delay(current: Duration) -> Duration {
    (current * 2).min(MAX_RESTART_DELAY)
}

/// Wait for log streaming tasks to write out remaining output. Bounded, since
/// a grandchild that inherited the pipes can keep them open indefinitely.
async fn drain_log_tasks(tasks: Vec<JoinHandle<()>>) {
    let _ = tokio::time::timeout(LOG_DRAIN_TIMEOUT, async {
        for task in tasks {
            let _ = task.await;
        }
    })
    .await;
}
