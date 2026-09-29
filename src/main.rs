mod cli;
mod config;
mod control;
mod error;
mod logs;
mod ports;
mod process;
mod state;
mod supervisor;
mod tui;

use crate::control::{StartOutcome, StopOutcome};
use crate::logs::LogHandler;
use crate::state::StateManager;
use crate::supervisor::Supervisor;
use anyhow::{Context, Result};
use clap::Parser;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::Path;
use tracing::Level;

fn main() -> Result<()> {
    // Parse CLI arguments
    let args = cli::Args::parse();

    match args.command {
        cli::Commands::Run {
            name,
            cmd,
            restart,
            env,
            detach,
            internal_supervisor,
        } => {
            if internal_supervisor {
                let _ = nix::unistd::setsid();
            }

            if let Some(bad) = env.iter().find(|e| !e.contains('=')) {
                anyhow::bail!("Invalid environment variable '{}': expected KEY=VALUE", bad);
            }

            if detach {
                // The background supervisor's stderr goes to /dev/null, so
                // report an already-running name here where the user sees it.
                let state_manager =
                    StateManager::new().context("Failed to initialize state manager")?;
                if let Some(existing) = control::running_process(&state_manager, &name) {
                    anyhow::bail!(
                        "Process '{}' is already running (PID: {})",
                        name,
                        existing.pid
                    );
                }

                match control::spawn_detached(
                    &state_manager,
                    &name,
                    &cmd,
                    restart,
                    &env,
                    None,
                    &HashMap::new(),
                )? {
                    StartOutcome::Started { .. } => {
                        println!("Process '{}' started in background.", name)
                    }
                    StartOutcome::Exited => println!(
                        "Process '{}' started and exited right away. See `runa logs {}`.",
                        name, name
                    ),
                    StartOutcome::Failed(reason) => {
                        anyhow::bail!("Process '{}' failed to start: {}", name, reason)
                    }
                }
                return Ok(());
            }

            run_supervisor(name, cmd, restart, env, internal_supervisor)?;
        }
        cli::Commands::Stop { name, all } => {
            let state_manager =
                StateManager::new().context("Failed to initialize state manager")?;

            if all {
                let processes = state_manager
                    .list_processes()
                    .context("Failed to list processes")?;
                if processes.is_empty() {
                    println!("No processes to stop.");
                } else {
                    for p in processes {
                        match control::request_stop(&state_manager, &p.name) {
                            Ok(StopOutcome::Signalled { pid }) => {
                                println!("Stopping process '{}' (PID: {})...", p.name, pid)
                            }
                            Ok(StopOutcome::WasDead) => println!(
                                "Process '{}' (PID: {}) is already dead. Cleaning up...",
                                p.name, p.pid
                            ),
                            Ok(StopOutcome::NotFound) => {}
                            Err(err) => eprintln!("Failed to stop '{}': {:#}", p.name, err),
                        }
                    }
                    println!("Stop all command issued.");
                }
            } else if let Some(name) = name {
                match control::request_stop(&state_manager, &name)? {
                    StopOutcome::Signalled { pid } => {
                        println!("Stopping process '{}' (PID: {})...", name, pid)
                    }
                    StopOutcome::WasDead => {
                        println!("Process '{}' is already dead. Cleaning up...", name)
                    }
                    StopOutcome::NotFound => anyhow::bail!("Process '{}' not found", name),
                }
            } else {
                anyhow::bail!("Please specify a process name or use --all");
            }
        }
        cli::Commands::Up { names, file } => cmd_up(&names, file.as_deref())?,
        cli::Commands::Down { names, file } => cmd_down(&names, file.as_deref())?,
        cli::Commands::Init => cmd_init()?,
        cli::Commands::Restart { name } => {
            let state_manager =
                StateManager::new().context("Failed to initialize state manager")?;
            let meta = state_manager
                .get_process(&name)
                .context(format!("Process '{}' not found", name))?;
            if state::is_runa_supervisor(meta.pid) {
                println!("Restarting process '{}' (PID: {})...", name, meta.pid);
                signal::kill(Pid::from_raw(meta.pid), Signal::SIGHUP)
                    .context("Failed to send SIGHUP")?;
            } else {
                anyhow::bail!("Process '{}' is not running.", name);
            }
        }
        cli::Commands::Logs {
            name,
            follow,
            lines,
        } => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;

            rt.block_on(async {
                let state_manager =
                    StateManager::new().context("Failed to initialize state manager")?;
                let run_dir = state_manager.get_run_dir();

                let stdout_path = logs::log_path(&run_dir, &name, "out");
                let stderr_path = logs::log_path(&run_dir, &name, "err");

                if !stdout_path.exists() && !stderr_path.exists() {
                    anyhow::bail!("No log files found for process '{}'", name);
                }

                if follow {
                    println!("Following stdout and stderr for '{}'...", name);
                    tokio::try_join!(
                        LogHandler::tail_logs(stdout_path, lines),
                        LogHandler::tail_logs(stderr_path, lines)
                    )
                    .context("Failed to tail logs")?;
                } else {
                    for (label, path) in [("stdout", &stdout_path), ("stderr", &stderr_path)] {
                        println!("--- {label} ---");
                        if path.exists() {
                            let bytes = std::fs::read(path)
                                .with_context(|| format!("Failed to read {label} log"))?;
                            let content = String::from_utf8_lossy(&bytes);
                            match lines {
                                Some(n) => print!("{}", logs::last_lines(&content, n)),
                                None => print!("{}", content),
                            }
                        }
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        cli::Commands::Flush { name } => {
            let state_manager =
                StateManager::new().context("Failed to initialize state manager")?;
            let run_dir = state_manager.get_run_dir();
            LogHandler::clear_logs(&name, run_dir);
            println!("Logs for process '{}' flushed.", name);
        }
        cli::Commands::Status { json } => {
            let state_manager =
                StateManager::new().context("Failed to initialize state manager")?;
            let rows = collect_status_rows(&state_manager)?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).context("Failed to serialize status")?
                );
            } else if rows.is_empty() {
                println!("No processes tracked.");
            } else {
                println!(
                    "{:<20} {:<10} {:<10} {:<9} {:<15} {:<20} {:<30}",
                    "NAME", "PID", "STATUS", "RESTARTS", "PORT", "STARTED", "CMD"
                );
                for row in rows {
                    let port_str = if row.ports.is_empty() {
                        "-".to_string()
                    } else {
                        row.ports
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let started_str = row.started_local.format("%Y-%m-%d %H:%M:%S").to_string();
                    let mut status_str = row.status.to_string();
                    status_str[..1].make_ascii_uppercase();

                    println!(
                        "{:<20} {:<10} {:<10} {:<9} {:<15} {:<20} {:<30}",
                        row.name, row.pid, status_str, row.restarts, port_str, started_str, row.cmd
                    );
                }
            }
        }
        cli::Commands::Watch {
            interval,
            udp,
            filter,
        } => {
            tui::run_watch(interval, udp, filter)?;
        }
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct StatusRow {
    name: String,
    running: bool,
    /// "running", "failed" (gave up, e.g. the command could not be spawned)
    /// or "dead" (the supervisor is gone).
    status: &'static str,
    pid: i32,
    child_pid: Option<i32>,
    cmd: String,
    cwd: Option<String>,
    ports: Vec<u16>,
    restarts: u64,
    last_exit_code: Option<i32>,
    last_error: Option<String>,
    started_at: String,
    #[serde(skip)]
    started_local: chrono::DateTime<chrono::Local>,
}

fn collect_status_rows(state_manager: &StateManager) -> Result<Vec<StatusRow>> {
    let mut processes = state_manager
        .list_processes()
        .context("Failed to list processes")?;
    processes.sort_by(|a, b| a.name.cmp(&b.name));

    let snapshot = ports::collect_snapshot(state_manager, false);
    let mut runa_ports: HashMap<String, Vec<u16>> = HashMap::new();
    for row in &snapshot.rows {
        if let Some(runa) = &row.runa {
            runa_ports
                .entry(runa.name.clone())
                .or_default()
                .push(row.entry.port);
        }
    }

    Ok(processes
        .into_iter()
        .map(|p| {
            let running = state::is_runa_supervisor(p.pid);
            let status = match (running, &p.status) {
                (true, _) => "running",
                (false, state::ProcessStatus::Failed) => "failed",
                (false, _) => "dead",
            };
            let mut ports = runa_ports.remove(&p.name).unwrap_or_default();
            ports.sort_unstable();
            ports.dedup();
            let started_local: chrono::DateTime<chrono::Local> =
                (std::time::UNIX_EPOCH + std::time::Duration::from_secs(p.created_at)).into();

            StatusRow {
                name: p.name,
                running,
                status,
                pid: p.pid,
                child_pid: p.child_pid,
                cmd: p.cmd,
                cwd: p.cwd,
                ports,
                restarts: p.restart_count,
                last_exit_code: p.last_exit_code,
                last_error: p.last_error,
                started_at: started_local.to_rfc3339(),
                started_local,
            }
        })
        .collect())
}

fn cmd_up(names: &[String], file: Option<&Path>) -> Result<()> {
    let loaded = config::load(file)?;
    let selected = loaded.select(names)?;
    let state_manager = StateManager::new().context("Failed to initialize state manager")?;
    let width = selected
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);

    let mut failed = 0;
    for (name, spec) in selected {
        match start_from_config(&loaded, &state_manager, name, spec) {
            Ok(message) => println!("  {name:<width$}  {message}"),
            Err(err) => {
                failed += 1;
                println!("  {name:<width$}  failed: {err:#}");
            }
        }
    }

    if failed > 0 {
        anyhow::bail!("{failed} process(es) failed to start");
    }
    Ok(())
}

fn start_from_config(
    loaded: &config::LoadedConfig,
    state_manager: &StateManager,
    name: &str,
    spec: &config::ProcessSpec,
) -> Result<String> {
    let cwd = loaded.cwd_for(name, spec)?;

    if let Some(existing) = control::running_process(state_manager, name) {
        // Names are global, so another project may already be using this one.
        if let Some(other) = existing.cwd.as_deref().filter(|dir| Path::new(dir) != cwd) {
            anyhow::bail!(
                "name is in use by a process running in {} (PID {})",
                other,
                existing.pid
            );
        }
        return Ok(format!("already running (PID {})", existing.pid));
    }

    let env = loaded.env_for(name, spec)?;
    match control::spawn_detached(
        state_manager,
        name,
        &spec.cmd,
        spec.restart,
        &[],
        Some(&cwd),
        &env,
    )? {
        StartOutcome::Started { pid } => Ok(format!("started (PID {pid})")),
        StartOutcome::Exited => Ok(format!(
            "started and exited right away (see `runa logs {name}`)"
        )),
        StartOutcome::Failed(reason) => anyhow::bail!("{reason}"),
    }
}

fn cmd_down(names: &[String], file: Option<&Path>) -> Result<()> {
    let loaded = config::load(file)?;
    let selected = loaded.select(names)?;
    let state_manager = StateManager::new().context("Failed to initialize state manager")?;
    let width = selected
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);

    // Signal everything first, then wait for all of them together.
    let mut results: Vec<(&str, Result<String, String>)> = Vec::new();
    let mut stopping: Vec<(usize, i32)> = Vec::new();
    for (name, spec) in &selected {
        // Don't stop another project's process that happens to share the name.
        if let Some(existing) = control::running_process(&state_manager, name)
            && let Ok(cwd) = loaded.cwd_for(name, spec)
            && let Some(other) = existing.cwd.as_deref().filter(|dir| Path::new(dir) != cwd)
        {
            results.push((
                name,
                Err(format!(
                    "skipped: name belongs to a process running in {other}"
                )),
            ));
            continue;
        }

        let result = match control::request_stop(&state_manager, name) {
            Ok(StopOutcome::Signalled { pid }) => {
                stopping.push((results.len(), pid));
                Ok(String::new())
            }
            Ok(StopOutcome::WasDead | StopOutcome::NotFound) => Ok("not running".to_string()),
            Err(err) => Err(format!("failed: {err:#}")),
        };
        results.push((name, result));
    }

    let pids: Vec<i32> = stopping.iter().map(|(_, pid)| *pid).collect();
    let still_running = control::wait_for_exit(&pids, control::STOP_WAIT_TIMEOUT);
    for (index, pid) in stopping {
        results[index].1 = if still_running.contains(&pid) {
            Err(format!("still running (PID {pid})"))
        } else {
            Ok("stopped".to_string())
        };
    }

    let mut failed = 0;
    for (name, result) in results {
        match result {
            Ok(message) => println!("  {name:<width$}  {message}"),
            Err(message) => {
                failed += 1;
                println!("  {name:<width$}  {message}");
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} process(es) were not stopped");
    }
    Ok(())
}

fn cmd_init() -> Result<()> {
    use std::io::Write;

    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(config::DEFAULT_FILE)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!("{} already exists", config::DEFAULT_FILE)
        }
        Err(err) => return Err(err).context(format!("Failed to create {}", config::DEFAULT_FILE)),
    };
    file.write_all(config::TEMPLATE.as_bytes())
        .context(format!("Failed to write {}", config::DEFAULT_FILE))?;
    println!(
        "Created {}. Edit it, then run `runa up`.",
        config::DEFAULT_FILE
    );
    Ok(())
}

fn run_supervisor(
    name: String,
    cmd: String,
    restart: cli::RestartPolicy,
    env: Vec<String>,
    quiet: bool,
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
        let mut supervisor =
            Supervisor::new(name, cmd, restart, env_vars).context("Failed to create supervisor")?;
        supervisor.run().await.context("Supervisor run error")?;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
