use crate::cli::RestartPolicy;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_FILE: &str = "runa.toml";

pub const TEMPLATE: &str = r#"# Processes for this project. Start them with `runa up`, stop them with
# `runa down`. Relative paths are resolved from this file's directory.

[processes.web]
cmd = "npm run dev"
# cwd = "./web"              # working directory (default: this file's directory)
# restart = "always"         # always | on-failure | never
# env = { PORT = 3000 }      # extra environment variables
# env_file = ".env"          # load variables from a dotenv file

# [processes.worker]
# cmd = "python3 worker.py"
# restart = "on-failure"
"#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub processes: BTreeMap<String, ProcessSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSpec {
    pub cmd: String,
    pub cwd: Option<PathBuf>,
    #[serde(default = "default_restart")]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub env: BTreeMap<String, toml::Value>,
    pub env_file: Option<PathBuf>,
}

fn default_restart() -> RestartPolicy {
    RestartPolicy::Always
}

pub struct LoadedConfig {
    pub path: PathBuf,
    pub base_dir: PathBuf,
    pub config: ProjectConfig,
}

/// Load `file`, or `./runa.toml` when no file is given.
pub fn load(file: Option<&Path>) -> Result<LoadedConfig> {
    let path = file
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE));
    let content = std::fs::read_to_string(&path).with_context(|| match file {
        Some(_) => format!("Failed to read {}", path.display()),
        None => format!("No {DEFAULT_FILE} in the current directory (create one with `runa init`)"),
    })?;
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("Failed to resolve {}", path.display()))?;
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    parse(&content, path, base_dir)
}

fn parse(content: &str, path: PathBuf, base_dir: PathBuf) -> Result<LoadedConfig> {
    let config: ProjectConfig =
        toml::from_str(content).with_context(|| format!("Invalid {}", path.display()))?;

    if config.processes.is_empty() {
        bail!(
            "{} defines no processes (add a [processes.<name>] table)",
            path.display()
        );
    }
    for (name, spec) in &config.processes {
        let parts = shell_words::split(&spec.cmd)
            .with_context(|| format!("Process '{name}': invalid cmd"))?;
        if parts.is_empty() {
            bail!("Process '{name}': cmd is empty");
        }
    }

    Ok(LoadedConfig {
        path,
        base_dir,
        config,
    })
}

impl LoadedConfig {
    /// The processes to act on: all of them when `names` is empty, otherwise
    /// the named ones in the order given.
    pub fn select(&self, names: &[String]) -> Result<Vec<(&str, &ProcessSpec)>> {
        if names.is_empty() {
            return Ok(self
                .config
                .processes
                .iter()
                .map(|(name, spec)| (name.as_str(), spec))
                .collect());
        }

        let unknown: Vec<&str> = names
            .iter()
            .filter(|name| !self.config.processes.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            let defined: Vec<&str> = self.config.processes.keys().map(String::as_str).collect();
            bail!(
                "Unknown process {} (defined in {}: {})",
                unknown.join(", "),
                self.path.display(),
                defined.join(", ")
            );
        }

        let mut seen = HashSet::new();
        Ok(names
            .iter()
            .filter(|name| seen.insert(name.as_str()))
            .filter_map(|name| self.config.processes.get_key_value(name))
            .map(|(name, spec)| (name.as_str(), spec))
            .collect())
    }

    /// Absolute working directory for a process.
    pub fn cwd_for(&self, name: &str, spec: &ProcessSpec) -> Result<PathBuf> {
        let dir = match &spec.cwd {
            Some(cwd) => self.base_dir.join(cwd),
            None => self.base_dir.clone(),
        };
        std::fs::canonicalize(&dir)
            .ok()
            .filter(|dir| dir.is_dir())
            .with_context(|| format!("Process '{name}': cwd {} is not a directory", dir.display()))
    }

    /// Environment for a process: `env_file` entries, overridden by `env`.
    pub fn env_for(&self, name: &str, spec: &ProcessSpec) -> Result<HashMap<String, String>> {
        let mut env = HashMap::new();

        if let Some(file) = &spec.env_file {
            let path = self.base_dir.join(file);
            let entries = dotenvy::from_path_iter(&path).with_context(|| {
                format!(
                    "Process '{name}': failed to read env_file {}",
                    path.display()
                )
            })?;
            for entry in entries {
                let (key, value) = entry.with_context(|| {
                    format!("Process '{name}': invalid line in {}", path.display())
                })?;
                env.insert(key, value);
            }
        }

        for (key, value) in &spec.env {
            let value = match value {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                _ => bail!("Process '{name}': env.{key} must be a string, number or boolean"),
            };
            env.insert(key.clone(), value);
        }

        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(content: &str) -> Result<LoadedConfig> {
        parse(
            content,
            PathBuf::from("/proj/runa.toml"),
            PathBuf::from("/proj"),
        )
    }

    #[test]
    fn parses_full_example() {
        let loaded = parse_str(
            r#"
            [processes.api]
            cmd = "bun run index.ts"
            cwd = "./api"
            restart = "on-failure"
            env = { PORT = 3000, DEBUG = true, NAME = "api", RATIO = 0.5 }
            env_file = ".env"

            [processes.worker]
            cmd = "python3 worker.py"
            "#,
        )
        .unwrap();

        let api = &loaded.config.processes["api"];
        assert_eq!(api.cmd, "bun run index.ts");
        assert_eq!(api.cwd.as_deref(), Some(Path::new("./api")));
        assert_eq!(api.restart, RestartPolicy::OnFailure);
        assert_eq!(api.env_file.as_deref(), Some(Path::new(".env")));

        let worker = &loaded.config.processes["worker"];
        assert_eq!(worker.restart, RestartPolicy::Always);
        assert!(worker.cwd.is_none());
        assert!(worker.env.is_empty());
    }

    #[test]
    fn stringifies_env_scalars() {
        let loaded = parse_str(
            r#"
            [processes.api]
            cmd = "node server.js"
            env = { PORT = 3000, DEBUG = true, NAME = "api", RATIO = 0.5 }
            "#,
        )
        .unwrap();
        let env = loaded
            .env_for("api", &loaded.config.processes["api"])
            .unwrap();
        assert_eq!(env["PORT"], "3000");
        assert_eq!(env["DEBUG"], "true");
        assert_eq!(env["NAME"], "api");
        assert_eq!(env["RATIO"], "0.5");
    }

    #[test]
    fn rejects_non_scalar_env_values() {
        let loaded = parse_str(
            r#"
            [processes.api]
            cmd = "node server.js"
            env = { LIST = [1, 2] }
            "#,
        )
        .unwrap();
        assert!(
            loaded
                .env_for("api", &loaded.config.processes["api"])
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_fields_and_bad_commands() {
        assert!(parse_str("[processes.api]\ncomand = \"node\"\n").is_err());
        assert!(parse_str("[processes.api]\ncmd = \"   \"\n").is_err());
        assert!(parse_str("[processes.api]\ncmd = \"echo 'unterminated\"\n").is_err());
        assert!(parse_str("[processes.api]\ncmd = \"node\"\nrestart = \"sometimes\"\n").is_err());
        assert!(parse_str("").is_err());
    }

    #[test]
    fn selects_processes_by_name() {
        let loaded = parse_str(
            "[processes.a]\ncmd = \"true\"\n[processes.b]\ncmd = \"true\"\n[processes.c]\ncmd = \"true\"\n",
        )
        .unwrap();

        let all: Vec<&str> = loaded
            .select(&[])
            .unwrap()
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(all, ["a", "b", "c"]);

        let names = vec!["c".to_string(), "a".to_string(), "c".to_string()];
        let some: Vec<&str> = loaded
            .select(&names)
            .unwrap()
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(some, ["c", "a"]);

        let err = loaded
            .select(&["nope".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn env_table_overrides_env_file() {
        let dir = std::env::temp_dir().join(format!("runa-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env"), "# comment\nFROM_FILE=file\nSHARED=file\n").unwrap();

        let loaded = parse(
            "[processes.api]\ncmd = \"true\"\nenv_file = \".env\"\nenv = { SHARED = \"table\" }\n",
            dir.join("runa.toml"),
            dir.clone(),
        )
        .unwrap();
        let env = loaded.env_for("api", &loaded.config.processes["api"]);
        let _ = std::fs::remove_dir_all(&dir);

        let env = env.unwrap();
        assert_eq!(env["FROM_FILE"], "file");
        assert_eq!(env["SHARED"], "table");
    }

    #[test]
    fn template_is_valid() {
        let loaded = parse_str(TEMPLATE).unwrap();
        assert!(loaded.config.processes.contains_key("web"));
    }
}
