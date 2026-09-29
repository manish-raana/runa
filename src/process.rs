use crate::error::{Result, RunaError};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct ProcessManager;

impl ProcessManager {
    pub fn spawn(cmd: &str, env_vars: &HashMap<String, String>) -> Result<Child> {
        let parts = shell_words::split(cmd)
            .map_err(|e| RunaError::InvalidCommand(format!("Failed to parse command: {}", e)))?;

        if parts.is_empty() {
            return Err(RunaError::InvalidCommand("Empty command".to_string()));
        }

        let program = &parts[0];
        let args = &parts[1..];

        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env_vars);

        // Put the child in its own process group
        unsafe {
            command.pre_exec(|| {
                let _ = nix::unistd::setpgid(
                    nix::unistd::Pid::from_raw(0),
                    nix::unistd::Pid::from_raw(0),
                );
                Ok(())
            });
        }

        let child = command
            .spawn()
            .map_err(|e| RunaError::Spawn(program.clone(), e))?;

        Ok(child)
    }
}
