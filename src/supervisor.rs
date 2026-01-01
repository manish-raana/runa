use crate::cli::RestartPolicy;
use crate::logs::LogHandler;
use crate::process::ProcessManager;
use crate::state::{ProcessMetadata, ProcessStatus, StateManager};
use crate::error::Result;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::signal;
use tokio::time::sleep;
use std::collections::HashMap;
use tokio::process::Child;
use tracing::{info, warn, error};

pub struct Supervisor {
    name: String,
    cmd: String,
    restart_policy: RestartPolicy,
    env_vars: HashMap<String, String>,
    state_manager: StateManager,
}

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
        
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        let mut sighup = signal::unix::signal(signal::unix::SignalKind::hangup()).unwrap();

        // Clear logs when the supervisor starts (Truncate policy)
        LogHandler::clear_logs(&self.name, self.state_manager.get_run_dir());

        loop {
            // Register process state
            let mut meta = ProcessMetadata {
                name: self.name.clone(),
                pid: supervisor_pid,
                child_pid: None,
                cmd: self.cmd.clone(),
                status: ProcessStatus::Running,
                created_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            };
            self.state_manager.save_process(&meta)?;

            // Spawn process
            info!(name = %self.name, cmd = %self.cmd, "Spawning child process...");
            let mut child = match ProcessManager::spawn(&self.cmd, &self.env_vars) {
                Ok(c) => c,
                Err(e) => {
                    error!(name = %self.name, error = %e, "Failed to spawn process");
                    self.update_status(ProcessStatus::Failed)?;
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
            if let Some(stdout) = child.stdout.take() {
                LogHandler::stream_stdout(stdout, self.name.clone(), log_dir.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                LogHandler::stream_stderr(stderr, self.name.clone(), log_dir);
            }

            // Wait for child exit or shutdown signal
            let status = tokio::select! {
                exit_status = child.wait() => {
                    exit_status?
                }
                _ = signal::ctrl_c() => {
                    info!(name = %self.name, "Received Ctrl+C, shutting down gracefully...");
                    self.graceful_shutdown(child).await;
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sigterm.recv() => {
                    info!(name = %self.name, "Received SIGTERM, shutting down gracefully...");
                    self.graceful_shutdown(child).await;
                    self.cleanup()?;
                    return Ok(());
                }
                _ = sighup.recv() => {
                    info!(name = %self.name, "Received SIGHUP, restarting process...");
                    self.graceful_shutdown(child).await;
                    // Loop will continue and restart process
                    continue;
                }
            };

            // Process exited
            if status.success() {
                info!(name = %self.name, code = status.code(), "Process exited successfully");
            } else {
                warn!(name = %self.name, code = status.code(), "Process exited with error");
            }
            
            match self.restart_policy {
                RestartPolicy::Always => {
                    info!(name = %self.name, "Restarting in 1 second (Policy: Always)...");
                    self.update_status(ProcessStatus::Running)?;
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                RestartPolicy::OnFailure => {
                    if !status.success() {
                        info!(name = %self.name, "Restarting in 1 second (Policy: OnFailure)...");
                        self.update_status(ProcessStatus::Running)?;
                        sleep(Duration::from_secs(1)).await;
                        continue;
                    } else {
                        info!(name = %self.name, "Process completed successfully, not restarting.");
                        self.update_status(ProcessStatus::Stopped)?;
                        break;
                    }
                }
                RestartPolicy::Never => {
                    info!(name = %self.name, "Process finished (Policy: Never).");
                    self.update_status(ProcessStatus::Stopped)?;
                    break;
                }
            }
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
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), nix::sys::signal::Signal::SIGTERM);
            
            let timeout = sleep(Duration::from_secs(5));
            tokio::select! {
                _ = child.wait() => {
                    info!(name = %self.name, "Child process exited.");
                }
                _ = timeout => {
                    warn!(name = %self.name, "Timeout reached, sending SIGKILL to process group...");
                    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), nix::sys::signal::Signal::SIGKILL);
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

    fn cleanup(&self) -> Result<()> {
        info!(name = %self.name, "Cleaning up process state...");
        self.state_manager.remove_process(&self.name)
    }
}
