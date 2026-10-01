mod cli;
mod config;
mod control;
mod error;
mod logs;
mod mcp;
mod metrics;
mod ports;
mod process;
mod project;
mod saved;
mod startup;
mod state;
mod status;
mod supervisor;
mod tui;
mod workspace;

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
        cli::Commands::Mcp => mcp::serve()?,
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
        cli::Commands::Ports { udp, all, json } => cmd_ports(udp, all, json)?,
        cli::Commands::Metrics { name, since, json } => cmd_metrics(&name, since, json)?,
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

fn cmd_ports(udp: bool, all: bool, json: bool) -> Result<()> {
    let mut snapshot = ports::collect_snapshot(&state_manager()?, udp);
    if !snapshot.errors.is_empty() && snapshot.rows.is_empty() {
        anyhow::bail!("{}", snapshot.errors.join("; "));
    }
    snapshot
        .rows
        .sort_by_key(|row| (row.entry.port, row.entry.pid));
    let pids: Vec<i32> = snapshot.rows.iter().map(|row| row.entry.pid).collect();
    let exes = ports::executable_paths(&pids);
    let cwds = workspace::process_cwds(&pids);
    let home = workspace::home_dir();
    let rows: Vec<ports::PortJson> = snapshot
        .rows
        .iter()
        .map(|row| {
            let mut json = ports::PortJson::new(row, exes.get(&row.entry.pid));
            if let Some(cwd) = cwds.get(&row.entry.pid) {
                json.project = workspace::detect(cwd, &home);
                json.cwd = Some(cwd.display().to_string());
            }
            json
        })
        .filter(|row| all || row.kind.is_dev())
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).context("Failed to serialize ports")?
        );
        return Ok(());
    }
    if rows.is_empty() {
        if all {
            println!("No listening ports.");
        } else {
            println!("No dev ports. Use --all to include system and app ports.");
        }
        return Ok(());
    }
    println!(
        "{:<6} {:<7} {:<16} {:<8} {:<20} {:<10} {:<24} {:<20}",
        "PROTO", "PORT", "ADDRESS", "PID", "COMMAND", "KIND", "PROJECT", "RUNA"
    );
    for row in &rows {
        let kind = serde_json::to_value(row.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let conflict = if row.conflict { " (conflict)" } else { "" };
        let project = row.project.as_ref().map_or("-".to_string(), |p| {
            let mut label = p.name.clone();
            if let Some(sub) = &p.sub {
                label = format!("{label}/{sub}");
            }
            if let Some(branch) = &p.branch {
                label = format!("{label}@{branch}");
            }
            label
        });
        println!(
            "{:<6} {:<7} {:<16} {:<8} {:<20} {:<10} {:<24} {}{}",
            row.protocol,
            row.port,
            row.address,
            row.pid,
            row.command,
            kind,
            project,
            row.runa.as_deref().unwrap_or("-"),
            conflict
        );
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct MetricsReport {
    name: String,
    interval_secs: u64,
    samples: Vec<metrics::Sample>,
    events: Vec<metrics::Event>,
}

fn cmd_metrics(name: &str, since: Option<u64>, json: bool) -> Result<()> {
    let state_manager = state_manager()?;
    let run_dir = state_manager.get_run_dir();
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let cutoff = since.map(|secs| now.saturating_sub(secs));
    let samples = metrics::read_samples(&run_dir, name, cutoff);
    let events = metrics::read_events(&run_dir, name, cutoff);
    if samples.is_empty() && events.is_empty() && state_manager.get_process(name).is_err() {
        anyhow::bail!("No metrics for process '{}'", name);
    }

    if json {
        let report = MetricsReport {
            name: name.to_string(),
            interval_secs: metrics::SAMPLE_INTERVAL.as_secs(),
            samples,
            events,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("Failed to serialize metrics")?
        );
        return Ok(());
    }

    match (samples.first(), samples.last()) {
        (Some(first), Some(last)) => {
            let n = samples.len() as f64;
            let avg_cpu = samples.iter().map(|s| s.cpu).sum::<f64>() / n;
            let max_cpu = samples.iter().map(|s| s.cpu).fold(0.0, f64::max);
            let avg_rss = samples.iter().map(|s| s.rss_kb).sum::<u64>() as f64 / n;
            let max_rss = samples.iter().map(|s| s.rss_kb).max().unwrap_or(0);
            println!(
                "{} samples over {}s",
                samples.len(),
                last.t.saturating_sub(first.t)
            );
            println!(
                "CPU     now {:>6.1}%   avg {:>6.1}%   peak {:>6.1}%",
                last.cpu, avg_cpu, max_cpu
            );
            println!(
                "Memory  now {:>7}   avg {:>7}   peak {:>7}",
                format_kb(last.rss_kb as f64),
                format_kb(avg_rss),
                format_kb(max_rss as f64)
            );
            println!("Procs   {}", last.procs);
        }
        _ => println!("No resource samples yet."),
    }

    if !events.is_empty() {
        println!("\nEvents:");
        for event in &events {
            let at: chrono::DateTime<chrono::Local> =
                (std::time::UNIX_EPOCH + std::time::Duration::from_secs(event.t)).into();
            let detail = match (event.event, event.code, event.pid) {
                (metrics::EventKind::Exit, Some(code), _) => format!("exit code {code}"),
                (metrics::EventKind::Exit, None, _) => "killed by signal".to_string(),
                (metrics::EventKind::Start, _, Some(pid)) => format!("pid {pid}"),
                _ => String::new(),
            };
            println!(
                "  {}  {:<8} {}",
                at.format("%Y-%m-%d %H:%M:%S"),
                format!("{:?}", event.event).to_lowercase(),
                detail
            );
        }
    }
    Ok(())
}

fn format_kb(kb: f64) -> String {
    if kb >= 1024.0 * 1024.0 {
        format!("{:.1} GB", kb / 1024.0 / 1024.0)
    } else if kb >= 1024.0 {
        format!("{:.1} MB", kb / 1024.0)
    } else {
        format!("{kb:.0} KB")
    }
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
