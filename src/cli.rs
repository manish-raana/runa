use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "runa")]
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

        /// Internal flag for detached supervisor
        #[arg(long, hide = true)]
        internal_supervisor: bool,
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
    },

    /// Clear logs for a process
    Flush {
        /// Name of the process
        #[arg(index = 1)]
        name: String,
    },

    /// List all processes
    Status,

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

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}
