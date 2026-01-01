use thiserror::Error;

#[derive(Error, Debug)]
pub enum RunaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("State error: {0}")]
    State(String),

    #[error("Process not found: {0}")]
    ProcessNotFound(String),

    #[error("Process already exists: {0}")]
    ProcessAlreadyExists(String),

    #[error("Invalid command: {0}")]
    InvalidCommand(String),
}

pub type Result<T> = std::result::Result<T, RunaError>;
