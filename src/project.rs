//! Starting and stopping the processes in a runa.toml. Returns per-process
//! reports instead of printing, so the CLI and the MCP server share it.

use crate::config::{LoadedConfig, ProcessSpec};
use crate::control::{self, SpawnSpec, StartOutcome, StopOutcome};
use crate::state::StateManager;
use anyhow::Result;
use std::path::Path;

/// The outcome for one process in a multi-process command.
pub struct Report {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

impl Report {
    pub fn ok(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            ok: true,
            message: message.into(),
        }
    }

    pub fn failed(name: &str, message: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            ok: false,
            message: message.into(),
        }
    }
}

/// One aligned line per process.
pub fn format_reports(reports: &[Report]) -> String {
    let width = reports.iter().map(|r| r.name.len()).max().unwrap_or(0);
    reports
        .iter()
        .map(|r| format!("  {:<width$}  {}\n", r.name, r.message))
        .collect()
}

pub fn failed_count(reports: &[Report]) -> usize {
    reports.iter().filter(|r| !r.ok).count()
}

/// Start the selected processes (all when `names` is empty).
pub fn up(
    state_manager: &StateManager,
    loaded: &LoadedConfig,
    names: &[String],
) -> Result<Vec<Report>> {
    Ok(loaded
        .select(names)?
        .into_iter()
        .map(
            |(name, spec)| match start_from_config(state_manager, loaded, name, spec) {
                Ok(message) => Report::ok(name, message),
                Err(err) => Report::failed(name, format!("failed: {err:#}")),
            },
        )
        .collect())
}

/// Start one process from a runa.toml unless it's already running.
pub fn start_from_config(
    state_manager: &StateManager,
    loaded: &LoadedConfig,
    name: &str,
    spec: &ProcessSpec,
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
    let outcome = control::spawn_detached(
        state_manager,
        &SpawnSpec {
            name,
            cmd: &spec.cmd,
            restart: spec.restart,
            cli_env: &[],
            cwd: Some(&cwd),
            extra_env: &env,
            config_file: Some(&loaded.path),
        },
    )?;
    describe_start(name, outcome)
}

/// Turn a start outcome into a report message, or an error if it failed.
pub fn describe_start(name: &str, outcome: StartOutcome) -> Result<String> {
    match outcome {
        StartOutcome::Started { pid } => Ok(format!("started (PID {pid})")),
        StartOutcome::Exited => Ok(format!(
            "started and exited right away (see `runa logs {name}`)"
        )),
        StartOutcome::Failed(reason) => anyhow::bail!("{reason}"),
    }
}

/// Stop the selected processes and wait for them to exit.
pub fn down(
    state_manager: &StateManager,
    loaded: &LoadedConfig,
    names: &[String],
) -> Result<Vec<Report>> {
    let selected = loaded.select(names)?;

    // Signal everything first, then wait for all of them together.
    let mut reports: Vec<Report> = Vec::new();
    let mut stopping: Vec<(usize, i32)> = Vec::new();
    for (name, spec) in &selected {
        // Don't stop another project's process that happens to share the name.
        if let Some(existing) = control::running_process(state_manager, name)
            && let Ok(cwd) = loaded.cwd_for(name, spec)
            && let Some(other) = existing.cwd.as_deref().filter(|dir| Path::new(dir) != cwd)
        {
            reports.push(Report::failed(
                name,
                format!("skipped: name belongs to a process running in {other}"),
            ));
            continue;
        }

        let report = match control::request_stop(state_manager, name) {
            Ok(StopOutcome::Signalled { pid }) => {
                stopping.push((reports.len(), pid));
                Report::ok(name, "")
            }
            Ok(StopOutcome::WasDead | StopOutcome::NotFound) => Report::ok(name, "not running"),
            Err(err) => Report::failed(name, format!("failed: {err:#}")),
        };
        reports.push(report);
    }

    let pids: Vec<i32> = stopping.iter().map(|(_, pid)| *pid).collect();
    let still_running = control::wait_for_exit(&pids, control::STOP_WAIT_TIMEOUT);
    for (index, pid) in stopping {
        let name = reports[index].name.clone();
        reports[index] = if still_running.contains(&pid) {
            Report::failed(&name, format!("still running (PID {pid})"))
        } else {
            Report::ok(&name, "stopped")
        };
    }

    Ok(reports)
}
