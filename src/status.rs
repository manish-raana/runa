use crate::metrics;
use crate::ports;
use crate::state::{self, StateManager};
use crate::workspace::{self, Workspace};
use anyhow::{Context, Result};
use std::collections::HashMap;

#[derive(serde::Serialize)]
pub struct StatusRow {
    pub name: String,
    pub running: bool,
    /// "running", "failed" (gave up, e.g. the command could not be spawned)
    /// or "dead" (the supervisor is gone).
    pub status: &'static str,
    pub pid: i32,
    pub child_pid: Option<i32>,
    pub cmd: String,
    pub cwd: Option<String>,
    /// The project `cwd` belongs to.
    pub project: Option<Workspace>,
    pub ports: Vec<u16>,
    pub restarts: u64,
    pub last_exit_code: Option<i32>,
    pub last_error: Option<String>,
    /// CPU use of the process group in percent of one core, if sampled recently.
    pub cpu: Option<f64>,
    /// Resident memory of the process group in KiB, if sampled recently.
    pub rss_kb: Option<u64>,
    pub started_at: String,
    #[serde(skip)]
    pub started_local: chrono::DateTime<chrono::Local>,
}

pub fn collect_status_rows(state_manager: &StateManager) -> Result<Vec<StatusRow>> {
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

    let run_dir = state_manager.get_run_dir();
    let home = workspace::home_dir();
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
            let sample = running
                .then(|| metrics::current_sample(&run_dir, &p.name))
                .flatten();

            StatusRow {
                name: p.name,
                running,
                status,
                pid: p.pid,
                child_pid: p.child_pid,
                cmd: p.cmd,
                project: p
                    .cwd
                    .as_deref()
                    .and_then(|cwd| workspace::detect(std::path::Path::new(cwd), &home)),
                cwd: p.cwd,
                ports,
                restarts: p.restart_count,
                last_exit_code: p.last_exit_code,
                last_error: p.last_error,
                cpu: sample.as_ref().map(|s| s.cpu),
                rss_kb: sample.as_ref().map(|s| s.rss_kb),
                started_at: started_local.to_rfc3339(),
                started_local,
            }
        })
        .collect())
}
