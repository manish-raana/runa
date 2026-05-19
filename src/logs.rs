use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::time::sleep;

use crate::error::Result;

const LOG_SIZE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB

pub struct LogHandler;

impl LogHandler {
    pub fn clear_logs(prefix: &str, log_dir: PathBuf) {
        let stdout_path = log_dir.join(format!("{}.out.log", prefix));
        let stderr_path = log_dir.join(format!("{}.err.log", prefix));
        let _ = std::fs::remove_file(stdout_path);
        let _ = std::fs::remove_file(stderr_path);

        // Also remove rotation files
        let _ = std::fs::remove_file(log_dir.join(format!("{}.out.log.bak", prefix)));
        let _ = std::fs::remove_file(log_dir.join(format!("{}.err.log.bak", prefix)));
    }

    pub fn stream_stdout(stdout: ChildStdout, prefix: String, log_dir: PathBuf) {
        let prefix = Arc::new(prefix);
        let log_file_path = log_dir.join(format!("{}.out.log", prefix));

        tokio::spawn(async move {
            let mut file = Self::open_log_file(&log_file_path).await;

            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let now = chrono::Local::now();
                let timestamp = now.format("%Y-%m-%d %H:%M:%S").to_string();
                let formatted = format!("[{}] [{}] {}\n", timestamp, prefix, line);
                print!("{}", formatted);

                if let Some(f) = file.as_mut() {
                    if let Ok(metadata) = f.metadata().await
                        && metadata.len() > LOG_SIZE_THRESHOLD
                    {
                        // Rotate
                        let _ = Self::rotate_log(&log_file_path).await;
                        file = Self::open_log_file(&log_file_path).await;
                    }

                    if let Some(f) = file.as_mut() {
                        let _ = f.write_all(formatted.as_bytes()).await;
                    }
                }
            }
        });
    }

    pub fn stream_stderr(stderr: ChildStderr, prefix: String, log_dir: PathBuf) {
        let prefix = Arc::new(prefix);
        let log_file_path = log_dir.join(format!("{}.err.log", prefix));

        tokio::spawn(async move {
            let mut file = Self::open_log_file(&log_file_path).await;

            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let now = chrono::Local::now();
                let timestamp = now.format("%Y-%m-%d %H:%M:%S").to_string();
                let formatted = format!("[{}] [{}] {}\n", timestamp, prefix, line);
                eprint!("{}", formatted);

                if let Some(f) = file.as_mut() {
                    if let Ok(metadata) = f.metadata().await
                        && metadata.len() > LOG_SIZE_THRESHOLD
                    {
                        // Rotate
                        let _ = Self::rotate_log(&log_file_path).await;
                        file = Self::open_log_file(&log_file_path).await;
                    }

                    if let Some(f) = file.as_mut() {
                        let _ = f.write_all(formatted.as_bytes()).await;
                    }
                }
            }
        });
    }

    async fn open_log_file(path: &PathBuf) -> Option<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .ok()
    }

    async fn rotate_log(path: &PathBuf) -> std::io::Result<()> {
        let mut bak_path = path.clone();
        bak_path.set_extension("log.bak");
        tokio::fs::rename(path, bak_path).await
    }

    pub async fn tail_logs(path: PathBuf) -> Result<()> {
        if !path.exists() {
            println!("Log file not found: {:?}", path);
            return Ok(());
        }

        // Print existing content
        let content = tokio::fs::read_to_string(&path).await?;
        print!("{}", content);
        let mut pos = content.len() as u64;

        // Follow
        loop {
            let metadata = tokio::fs::metadata(&path).await?;
            let len = metadata.len();

            if len > pos {
                let mut file = File::open(&path).await?;
                file.seek(std::io::SeekFrom::Start(pos)).await?;

                let mut reader = BufReader::new(file);
                let mut line = String::new();
                while reader.read_line(&mut line).await? > 0 {
                    print!("{}", line);
                    line.clear();
                }
                pos = len;
            } else if len < pos {
                // File truncated or rotated
                pos = 0;
            }

            sleep(Duration::from_millis(500)).await;
        }
    }
}
