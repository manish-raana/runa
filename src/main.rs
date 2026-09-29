mod cli;
mod config;
mod control;
mod error;
mod logs;
mod ports;
mod process;
mod project;
mod saved;
mod startup;
mod state;
mod status;
mod supervisor;
mod tui;

use crate::control::{SpawnSpec, StartOutcome, StopOutcome};
use crate::logs::LogHandler;
use crate::project::Report;
use crate::state::StateManager;
use crate::supervisor::Supervisor;
use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
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
        } => {
            // Set by spawn_detached on background supervisors.
            let internal_supervisor = std::env::var_os(control::SUPERVISOR_ENV).is_some();
            if internal_supervisor {
                let _ = nix::unistd::setsid();
            }

            if let Some(bad) = env.iter().find(|e| !e.contains('=')) {
                anyhow::bail!("Invalid environment variable '{}': expected KEY=VALUE", bad);
            }

            if detach && !internal_supervisor {
                // The background supervisor's stderr goes to /dev/null, so
                // report an already-running name here where the user sees it.
                let state_manager = state_manager()?;
                if let Some(existing) = control::running_process(&state_manager, &name) {
                    anyhow::bail!(
                        "Process '{}' is already running (PID: {})",
                        name,
                        existing.pid
                    );
                }

                let outcome = control::spawn_detached(
                    &state_manager,
                    &SpawnSpec {
                        name: &name,
                        cmd: &cmd,
                        restart,
                        cli_env: &env,
                        cwd: None,
                        extra_env: &HashMap::new(),
                        config_file: None,
                    },
                )?;
                match outcome {
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

            let config_file = std::env::var(control::CONFIG_ENV).ok();
            run_supervisor(name, cmd, restart, env, config_file, internal_supervisor)?;
        }
        cli::Commands::Stop { name, all } => {
            let state_manager = state_manager()?;

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
        cli::Commands::Up { names, file } => {
            let loaded = config::load(file.as_deref())?;
            let reports = project::up(&state_manager()?, &loaded, &names)?;
            print_reports(&reports, "failed to start")?;
        }
        cli::Commands::Down { names, file } => {
            let loaded = config::load(file.as_deref())?;
            let reports = project::down(&state_manager()?, &loaded, &names)?;
            print_reports(&reports, "were not stopped")?;
        }
        cli::Commands::Init => cmd_init()?,
        cli::Commands::Save => {
            let saved = saved::save(&state_manager()?)?;
            if saved.is_empty() {
                println!("No running processes. Saved an empty list.");
            } else {
                let names: Vec<&str> = saved.iter().map(|p| p.name.as_str()).collect();
                println!(
                    "Saved {} process(es): {}. Restore them with `runa resurrect`.",
                    saved.len(),
                    names.join(", ")
                );
            }
        }
        cli::Commands::Resurrect => {
            let reports = saved::resurrect(&state_manager()?)?;
            if reports.is_empty() {
                println!("The saved list is empty.");
            }
            print_reports(&reports, "failed to start")?;
        }
        cli::Commands::Startup { remove } => {
            let message = if remove {
                startup::uninstall()?
            } else {
                let message = startup::install()?;
                if saved::load(&state_manager()?).is_err() {
                    println!(
                        "Note: nothing is saved yet. Run `runa save` once your processes are up."
                    );
                }
                message
            };
            println!("{message}");
        }
        cli::Commands::Completions { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(
                shell,
                &mut cli::Args::command(),
                "runa",
                &mut std::io::stdout(),
            );
        }
        cli::Commands::Restart { name } => {
            let pid = control::request_restart(&state_manager()?, &name)?;
            println!("Restarting process '{}' (PID: {})...", name, pid);
        }
        cli::Commands::Logs {
            name,
            follow,
            lines,
        } => {
            let run_dir = state_manager()?.get_run_dir();
            if !follow {
                print!("{}", logs::read_logs(&run_dir, &name, lines)?);
                return Ok(());
            }

            let stdout_path = logs::log_path(&run_dir, &name, "out");
            let stderr_path = logs::log_path(&run_dir, &name, "err");
            if !stdout_path.exists() && !stderr_path.exists() {
                anyhow::bail!("No log files found for process '{}'", name);
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;
            println!("Following stdout and stderr for '{}'...", name);
            rt.block_on(async {
                tokio::try_join!(
                    LogHandler::tail_logs(stdout_path, lines),
                    LogHandler::tail_logs(stderr_path, lines)
                )
            })
            .context("Failed to tail logs")?;
        }
        cli::Commands::Flush { name } => {
            let run_dir = state_manager()?.get_run_dir();
            LogHandler::clear_logs(&name, run_dir);
            println!("Logs for process '{}' flushed.", name);
        }
        cli::Commands::Status { json } => {
            let rows = status::collect_status_rows(&state_manager()?)?;

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

fn state_manager() -> Result<StateManager> {
    StateManager::new().context("Failed to initialize state manager")
}

/// Print one line per process; error if any failed.
fn print_reports(reports: &[Report], failure: &str) -> Result<()> {
    print!("{}", project::format_reports(reports));
    let failed = project::failed_count(reports);
    if failed > 0 {
        anyhow::bail!("{failed} process(es) {failure}");
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
    config_file: Option<String>,
    quiet: bool,
) -> Result<()> {
    if !quiet {
        // Initialize logging to console only if not detached
        let _ = tracing_subscriber::fmt()
            .with_max_level(Level::INFO)
            .try_init();
    }

    let mut env_vars = HashMap::new();
    for e in env {
        if let Some((k, v)) = e.split_once('=') {
            env_vars.insert(k.to_string(), v.to_string());
        }
    }

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(async {
        let mut supervisor = Supervisor::new(name, cmd, restart, env_vars, config_file)
            .context("Failed to create supervisor")?;
        supervisor.run().await.context("Supervisor run error")?;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
