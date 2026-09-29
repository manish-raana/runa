use crate::error::{Result, RunaError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ProcessStatus {
    Running,
    Stopped,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessMetadata {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    pub name: String,
    pub pid: i32,               // Supervisor PID
    pub child_pid: Option<i32>, // Task PID
    pub cmd: String,
    pub status: ProcessStatus,
    pub created_at: u64,
    #[serde(default)]
    pub restart_count: u64,
    #[serde(default)]
    pub last_exit_code: Option<i32>,
    #[serde(default)]
    pub last_exit_at: Option<u64>,
}

pub struct StateManager {
    run_dir: PathBuf,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let home =
            std::env::var("HOME").map_err(|_| RunaError::State("HOME not set".to_string()))?;
        let run_dir = Path::new(&home).join(".runa");

        if !run_dir.exists() {
            fs::create_dir_all(&run_dir)?;
        }

        Ok(Self { run_dir })
    }

    pub fn get_run_dir(&self) -> PathBuf {
        self.run_dir.clone()
    }

    pub fn save_process(&self, meta: &ProcessMetadata) -> Result<()> {
        let file_path = self.process_path(&meta.name);

        // Safety: never save invalid PIDs
        if meta.pid <= 1 {
            return Err(RunaError::State(format!("Invalid PID: {}", meta.pid)));
        }

        // In v0.1, we want to prevent starting a process if it's already "Running"
        if let Ok(existing) = self.get_process(&meta.name) {
            // Check if the supervisor process is actually alive AND it's not us
            if existing.pid != meta.pid && is_runa_supervisor(existing.pid) {
                return Err(RunaError::ProcessAlreadyExists(meta.name.clone()));
            }
        }

        let content = serde_json::to_string_pretty(meta)
            .map_err(|e| RunaError::State(format!("Serialization error: {}", e)))?;
        let tmp_path = file_path.with_extension("json.tmp");
        fs::write(&tmp_path, content)?;
        fs::rename(tmp_path, file_path)?;
        Ok(())
    }

    pub fn get_process(&self, name: &str) -> Result<ProcessMetadata> {
        let file_path = self.process_path(name);
        if !file_path.exists() {
            return Err(RunaError::ProcessNotFound(name.to_string()));
        }

        let content = fs::read_to_string(file_path)?;
        let meta: ProcessMetadata = serde_json::from_str(&content)
            .map_err(|e| RunaError::State(format!("Deserialization error: {}", e)))?;

        Ok(meta)
    }

    pub fn remove_process(&self, name: &str) -> Result<()> {
        let file_path = self.process_path(name);
        if file_path.exists() {
            fs::remove_file(file_path)?;
        }
        Ok(())
    }

    pub fn list_processes(&self) -> Result<Vec<ProcessMetadata>> {
        let mut processes = Vec::new();

        for entry in fs::read_dir(&self.run_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip if not a file or not .json
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            match fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(meta) = serde_json::from_str::<ProcessMetadata>(&content) {
                        processes.push(meta);
                    }
                }
                Err(e) => {
                    // Log error but continue
                    eprintln!("Warning: Failed to read state file {:?}: {}", path, e);
                }
            }
        }

        Ok(processes)
    }

    fn process_path(&self, name: &str) -> PathBuf {
        self.run_dir
            .join(format!("{}.json", sanitize_process_name(name)))
    }
}

fn current_schema_version() -> u32 {
    1
}

/// Returns true if `pid` is alive and is still a Runa supervisor. A stale state
/// file can point at a PID the OS has since reused for an unrelated process, so
/// liveness alone is not enough before sending signals.
pub fn is_runa_supervisor(pid: i32) -> bool {
    if pid <= 1 || nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
        return false;
    }

    let output = match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
    {
        Ok(output) => output,
        // ps unavailable: fall back to liveness only
        Err(_) => return true,
    };
    if !output.status.success() {
        return false;
    }

    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "runa".to_string());
    let args = String::from_utf8_lossy(&output.stdout);
    args.split_whitespace()
        .next()
        .and_then(|argv0| Path::new(argv0).file_name())
        .is_some_and(|name| name.to_string_lossy() == exe_name)
}

pub fn sanitize_process_name(name: &str) -> String {
    let mut sanitized = String::new();
    for byte in name.bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            sanitized.push(c);
        } else {
            sanitized.push_str(&format!("%{byte:02X}"));
        }
    }
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_process_name;

    #[test]
    fn sanitizes_process_names_for_filenames() {
        assert_eq!(sanitize_process_name("api"), "api");
        assert_eq!(sanitize_process_name("../api worker"), "..%2Fapi%20worker");
        assert_eq!(sanitize_process_name(""), "_");
    }
}
