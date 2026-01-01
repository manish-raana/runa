mod cli;
mod error;
mod logs;
mod process;
mod state;
mod supervisor;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::Level;
use crate::supervisor::Supervisor;
use crate::state::StateManager;
use crate::logs::LogHandler;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::process::Stdio;

fn main() -> Result<()> {
    // Parse CLI arguments
    let args = cli::Args::parse();

    match args.command {
        cli::Commands::Run { name, cmd, restart, env, detach, internal_supervisor } => {
            if internal_supervisor {
                let _ = nix::unistd::setsid();
            }

            if detach {
                let exe = std::env::current_exe().context("Failed to get current executable path")?;
                let mut command = std::process::Command::new(exe);
                command.arg("run")
                    .arg("--name").arg(&name)
                    .arg("--cmd").arg(&cmd)
                    .arg("--restart").arg(match restart {
                        cli::RestartPolicy::Always => "always",
                        cli::RestartPolicy::OnFailure => "onfailure",
                        cli::RestartPolicy::Never => "never",
                    })
                    .arg("--internal-supervisor");
                
                for e in &env {
                    command.arg("--env").arg(e);
                }

                // Redirect to avoid keeping terminal open
                command.stdin(Stdio::null());
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());

                command.spawn().context("Failed to spawn background supervisor")?;
                println!("Process '{}' started in background.", name);
                return Ok(());
            }

            run_supervisor(name, cmd, restart, env, internal_supervisor)?;
        }
        cli::Commands::Stop { name, all } => {
            let state_manager = StateManager::new().context("Failed to initialize state manager")?;
            let my_pid = std::process::id() as i32;

            if all {
                let processes = state_manager.list_processes().context("Failed to list processes")?;
                if processes.is_empty() {
                    println!("No processes to stop.");
                } else {
                    for p in processes {
                        if p.pid <= 1 || p.pid == my_pid {
                            continue;
                        }

                        if signal::kill(Pid::from_raw(p.pid), None).is_ok() {
                            println!("Stopping process '{}' (PID: {})...", p.name, p.pid);
                            let _ = signal::kill(Pid::from_raw(p.pid), Signal::SIGTERM);
                        } else {
                            println!("Process '{}' (PID: {}) is already dead. Cleaning up...", p.name, p.pid);
                            let _ = state_manager.remove_process(&p.name);
                        }
                    }
                    println!("Stop all command issued.");
                }
            } else if let Some(name) = name {
                let meta = state_manager.get_process(&name).context(format!("Process '{}' not found", name))?;
                if meta.pid <= 1 || meta.pid == my_pid {
                    anyhow::bail!("Invalid PID {} for process '{}'", meta.pid, name);
                }

                if signal::kill(Pid::from_raw(meta.pid), None).is_ok() {
                    println!("Stopping process '{}' (PID: {})...", name, meta.pid);
                    signal::kill(Pid::from_raw(meta.pid), Signal::SIGTERM).context("Failed to send SIGTERM")?;
                } else {
                    println!("Process '{}' (PID: {}) is already dead. Cleaning up...", name, meta.pid);
                    state_manager.remove_process(&name).context("Failed to remove process state")?;
                }
            } else {
                anyhow::bail!("Please specify a process name or use --all");
            }
        }
        cli::Commands::Restart { name } => {
            let state_manager = StateManager::new().context("Failed to initialize state manager")?;
            let meta = state_manager.get_process(&name).context(format!("Process '{}' not found", name))?;
            if meta.pid > 1 && signal::kill(Pid::from_raw(meta.pid), None).is_ok() {
                println!("Restarting process '{}' (PID: {})...", name, meta.pid);
                signal::kill(Pid::from_raw(meta.pid), Signal::SIGHUP).context("Failed to send SIGHUP")?;
            } else {
                anyhow::bail!("Process '{}' is not running.", name);
            }
        }
        cli::Commands::Logs { name, follow } => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build().context("Failed to build tokio runtime")?;
            
            rt.block_on(async {
                let state_manager = StateManager::new().context("Failed to initialize state manager")?;
                let run_dir = state_manager.get_run_dir();
                
                let stdout_path = run_dir.join(format!("{}.out.log", name));
                let stderr_path = run_dir.join(format!("{}.err.log", name));

                if !stdout_path.exists() && !stderr_path.exists() {
                    anyhow::bail!("No log files found for process '{}'", name);
                }

                if follow {
                    println!("Following stdout for '{}'...", name);
                    LogHandler::tail_logs(stdout_path).await.context("Failed to tail logs")?;
                } else {
                    println!("--- stdout ---");
                    if stdout_path.exists() {
                        let content = std::fs::read_to_string(&stdout_path).context("Failed to read stdout log")?;
                        print!("{}", content);
                    }

                    println!("\n--- stderr ---");
                    if stderr_path.exists() {
                        let content = std::fs::read_to_string(&stderr_path).context("Failed to read stderr log")?;
                        print!("{}", content);
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        cli::Commands::Flush { name } => {
            let state_manager = StateManager::new().context("Failed to initialize state manager")?;
            let run_dir = state_manager.get_run_dir();
            LogHandler::clear_logs(&name, run_dir);
            println!("Logs for process '{}' flushed.", name);
        }
        cli::Commands::Status => {
            let state_manager = StateManager::new().context("Failed to initialize state manager")?;
            let processes = state_manager.list_processes().context("Failed to list processes")?;
            
            // Get port map
            let mut port_map: HashMap<i32, Vec<String>> = HashMap::new();
            if let Ok(output) = std::process::Command::new("lsof")
                .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
                .output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines().skip(1) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 9 {
                        if let Ok(pid) = parts[1].parse::<i32>() {
                            let name = parts[8];
                            if let Some(port) = name.split(':').last() {
                                port_map.entry(pid).or_default().push(port.to_string());
                            }
                        }
                    }
                }
            }

            if processes.is_empty() {
                println!("No processes tracked.");
            } else {
                println!("{:<20} {:<10} {:<10} {:<15} {:<20} {:<30}", "NAME", "PID", "STATUS", "PORT", "STARTED", "CMD");
                for p in processes {
                    let alive = if p.pid > 1 {
                        signal::kill(Pid::from_raw(p.pid), None).is_ok()
                    } else {
                        false
                    };
                    let status_str = if alive { "Running" } else { "Dead" };
                    
                    // Ports - check supervisor and descendants if we can find them
                    // For now, we'll check all PIDs and see if their parent is this supervisor
                    // But simpler: just check the supervisor PID itself (if it binds) 
                    // or its child.
                    // Actually, let's just check ALL ports and see if any PID matches? 
                    // No, we need to know which PID belongs to which task.
                    
                    // For v0.1, we'll try to find ports for the supervisor PID 
                    // AND we'll do a quick check for child processes.
                    let mut ports = port_map.get(&p.pid).cloned().unwrap_or_default();
                    
                    // Also check for the task process and its entire group
                    if let Some(child_pid) = p.child_pid {
                        if let Some(p_ports) = port_map.get(&child_pid) {
                            ports.extend(p_ports.clone());
                        }

                        // Check the whole process group (pgid == child_pid)
                        if let Ok(output) = std::process::Command::new("pgrep")
                            .args(["-g", &child_pid.to_string()])
                            .output() {
                            let members = String::from_utf8_lossy(&output.stdout);
                            for member_pid_str in members.lines() {
                                if let Ok(member_pid) = member_pid_str.parse::<i32>() {
                                    if let Some(p_ports) = port_map.get(&member_pid) {
                                        ports.extend(p_ports.clone());
                                    }
                                }
                            }
                        }
                    }
                    
                    ports.sort();
                    ports.dedup();
                    
                    let port_str = if ports.is_empty() { "-".to_string() } else { ports.join(",") };
                    
                    let started_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(p.created_at);
                    let datetime: chrono::DateTime<chrono::Local> = started_at.into();
                    let started_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();

                    println!("{:<20} {:<10} {:<10} {:<15} {:<20} {:<30}", p.name, p.pid, status_str, port_str, started_str, p.cmd);
                }
            }
        }
    }

    Ok(())
}

fn run_supervisor(
    name: String, 
    cmd: String, 
    restart: cli::RestartPolicy, 
    env: Vec<String>,
    quiet: bool
) -> Result<()> {
    if !quiet {
        // Initialize logging to console only if not detached
        let _ = tracing_subscriber::fmt()
            .with_max_level(Level::INFO)
            .try_init();
    } else {
        // In background mode, we could log to a file, but for v0.1 we rely on child logs
    }

    let mut env_vars = HashMap::new();
    for e in env {
        if let Some((k, v)) = e.split_once('=') {
            env_vars.insert(k.to_string(), v.to_string());
        }
    }

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(async {
        let mut supervisor = Supervisor::new(name, cmd, restart, env_vars).context("Failed to create supervisor")?;
        supervisor.run().await.context("Supervisor run error")?;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
