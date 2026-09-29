use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// ASCII-art brand banner, shown at the top of `runa` help output.
pub const BANNER: &str = r"
 ____
|  _ \    _   _    _ __      __ _
| |_) |  | | | |  | '_ \    / _` |
|  _ <   | |_| |  | | | |  | (_| |
|_| \_\   \__,_|  |_| |_|   \__,_|

 keep your processes alive
";

#[derive(Parser, Debug)]
#[command(name = "runa", version, before_help = BANNER, arg_required_else_help = true)]
#[command(about = "A minimal process supervisor", long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start a new process
    Run {
        /// Name of the process
        #[arg(long)]
        name: String,

        /// Command to execute
        #[arg(long)]
        cmd: String,

        /// Restart policy
        #[arg(long, default_value = "always")]
        restart: RestartPolicy,

        /// Environment variables (KEY=VALUE)
        #[arg(long, short = 'e')]
        env: Vec<String>,

        /// Run in background (detach)
        #[arg(long, short = 'd')]
        detach: bool,
    },

    /// Stop a running process
    Stop {
        /// Name of the process to stop, or use --all
        #[arg(index = 1)]
        name: Option<String>,

        /// Stop all running processes
        #[arg(long, short = 'a')]
        all: bool,
    },

    /// Restart a process
    Restart {
        /// Name of the process
        #[arg(index = 1)]
        name: String,
    },

    /// View logs for a process
    Logs {
        /// Name of the process
        #[arg(index = 1)]
        name: String,

        /// Follow the logs in real-time
        #[arg(long, short = 'f')]
        follow: bool,

        /// Only show the last N lines of each log
        #[arg(long, short = 'n')]
        lines: Option<usize>,
    },

    /// Clear logs for a process
    Flush {
        /// Name of the process
        #[arg(index = 1)]
        name: String,
    },

    /// List all processes
    Status {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Start the processes defined in runa.toml
    Up {
        /// Processes to start (default: all)
        names: Vec<String>,

        /// Config file (default: ./runa.toml)
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
    },

    /// Stop the processes defined in runa.toml
    Down {
        /// Processes to stop (default: all)
        names: Vec<String>,

        /// Config file (default: ./runa.toml)
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
    },

    /// Create a runa.toml template in the current directory
    Init,

    /// Save the running processes so `runa resurrect` can start them again
    Save,

    /// Start the processes saved with `runa save`
    Resurrect,

    /// Start saved processes automatically at login (launchd/systemd)
    Startup {
        /// Remove the login item instead
        #[arg(long)]
        remove: bool,
    },

    /// Run an MCP server on stdin/stdout so AI agents can manage processes
    Mcp,

    /// Print a shell completion script (e.g. `runa completions zsh`)
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },

    /// Watch local ports in a live terminal dashboard
    Watch {
        /// Refresh interval in milliseconds
        #[arg(long, default_value_t = 1000)]
        interval: u64,

        /// Include UDP sockets
        #[arg(long)]
        udp: bool,

        /// Initial text filter
        #[arg(long)]
        filter: Option<String>,
    },
}

#[derive(
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    ValueEnum,
    Debug,
    serde::Deserialize,
    serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

impl RestartPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::OnFailure => "on-failure",
            Self::Never => "never",
        }
    }
}
