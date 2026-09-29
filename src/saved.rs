//! `runa save` / `runa resurrect`: snapshot the running processes and start
//! them again later (for example at login, via `runa startup`).

use crate::cli::RestartPolicy;
use crate::config;
use crate::control::{self, SpawnSpec};
use crate::project::{self, Report};
use crate::state::{self, StateManager};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedProcess {
    pub name: String,
    pub cmd: String,
    pub restart: RestartPolicy,
    /// `KEY=VALUE` pairs given with `--env`.
    #[serde(default)]
    pub env: Vec<String>,
    pub cwd: Option<String>,
    /// Processes from a runa.toml are restored from that file, so env_file
    /// values are re-read rather than copied into the snapshot.
    pub config_file: Option<String>,
}

/// Lives in a subdirectory so it can't collide with a process's
/// `<name>.json` state file.
fn saved_path(state_manager: &StateManager) -> PathBuf {
    state_manager
        .get_run_dir()
        .join("saved")
        .join("processes.json")
}

/// Record every running process. Returns what was saved.
pub fn save(state_manager: &StateManager) -> Result<Vec<SavedProcess>> {
    let mut processes = state_manager
        .list_processes()
        .context("Failed to list processes")?;
    processes.retain(|p| state::is_runa_supervisor(p.pid));
    processes.sort_by(|a, b| a.name.cmp(&b.name));

    let saved: Vec<SavedProcess> = processes
        .into_iter()
        .map(|p| SavedProcess {
            name: p.name,
            cmd: p.cmd,
            // Supervisors from before v0.3 don't record their policy.
            restart: p.restart.unwrap_or(RestartPolicy::Always),
            env: p.env,
            cwd: p.cwd,
            config_file: p.config_file,
        })
        .collect();

    let path = saved_path(state_manager);
    let dir = path.parent().expect("saved path has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let content = serde_json::to_string_pretty(&saved).context("Failed to serialize")?;
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, content.as_bytes())?;
    std::fs::rename(&tmp, &path).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(saved)
}

fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    file.write_all(content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

pub fn load(state_manager: &StateManager) -> Result<Vec<SavedProcess>> {
    let path = saved_path(state_manager);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("Nothing saved yet. Start your processes, then run `runa save`.")
        }
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", path.display())),
    };
    serde_json::from_str(&content).with_context(|| format!("Invalid {}", path.display()))
}

/// Start every saved process that isn't already running.
pub fn resurrect(state_manager: &StateManager) -> Result<Vec<Report>> {
    Ok(load(state_manager)?
        .iter()
        .map(|saved| match restore(state_manager, saved) {
            Ok(message) => Report::ok(&saved.name, message),
            Err(err) => Report::failed(&saved.name, format!("failed: {err:#}")),
        })
        .collect())
}

fn restore(state_manager: &StateManager, saved: &SavedProcess) -> Result<String> {
    if let Some(existing) = control::running_process(state_manager, &saved.name) {
        return Ok(format!("already running (PID {})", existing.pid));
    }

    if let Some(file) = &saved.config_file {
        let loaded = config::load(Some(Path::new(file)))?;
        let (name, spec) = loaded
            .config
            .processes
            .get_key_value(&saved.name)
            .with_context(|| format!("{file} no longer defines this process"))?;
        return project::start_from_config(state_manager, &loaded, name, spec);
    }

    let cwd = saved.cwd.as_deref().map(Path::new);
    if let Some(cwd) = cwd
        && !cwd.is_dir()
    {
        anyhow::bail!("working directory {} no longer exists", cwd.display());
    }
    let outcome = control::spawn_detached(
        state_manager,
        &SpawnSpec {
            name: &saved.name,
            cmd: &saved.cmd,
            restart: saved.restart,
            cli_env: &saved.env,
            cwd,
            extra_env: &HashMap::new(),
            config_file: None,
        },
    )?;
    project::describe_start(&saved.name, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_process_round_trips() {
        let saved = vec![SavedProcess {
            name: "api".into(),
            cmd: "node server.js".into(),
            restart: RestartPolicy::OnFailure,
            env: vec!["PORT=3000".into()],
            cwd: Some("/proj".into()),
            config_file: None,
        }];
        let json = serde_json::to_string(&saved).unwrap();
        assert!(json.contains("\"on-failure\""), "{json}");
        let back: Vec<SavedProcess> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
    }
}
