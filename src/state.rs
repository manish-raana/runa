use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::fs;
use crate::error::{Result, RunaError};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ProcessStatus {
    Running,
    Stopped,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessMetadata {
    pub name: String,
    pub pid: i32, // Supervisor PID
    pub child_pid: Option<i32>, // Task PID
    pub cmd: String,
    pub status: ProcessStatus,
    pub created_at: u64,
}

pub struct StateManager {
    run_dir: PathBuf,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME").map_err(|_| RunaError::State("HOME not set".to_string()))?;
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
        let file_path = self.run_dir.join(format!("{}.json", meta.name));
        
        // Safety: never save invalid PIDs
        if meta.pid <= 1 {
            return Err(RunaError::State(format!("Invalid PID: {}", meta.pid)));
        }

        // In v0.1, we want to prevent starting a process if it's already "Running"
        if let Ok(existing) = self.get_process(&meta.name) {
            // Check if the supervisor process is actually alive AND it's not us
            let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(existing.pid as i32), None).is_ok();
            if alive && existing.pid != meta.pid {
                return Err(RunaError::ProcessAlreadyExists(meta.name.clone()));
            }
        }

        let content = serde_json::to_string_pretty(meta)
            .map_err(|e| RunaError::State(format!("Serialization error: {}", e)))?;
        fs::write(file_path, content)?;
        Ok(())
    }

    pub fn get_process(&self, name: &str) -> Result<ProcessMetadata> {
        let file_path = self.run_dir.join(format!("{}.json", name));
        if !file_path.exists() {
            return Err(RunaError::ProcessNotFound(name.to_string()));
        }
        
        let content = fs::read_to_string(file_path)?;
        let meta: ProcessMetadata = serde_json::from_str(&content)
            .map_err(|e| RunaError::State(format!("Deserialization error: {}", e)))?;
        
        Ok(meta)
    }

    pub fn remove_process(&self, name: &str) -> Result<()> {
        let file_path = self.run_dir.join(format!("{}.json", name));
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
}
