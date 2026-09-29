use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::error::Result;
use crate::state::sanitize_process_name;

const LOG_SIZE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB

/// Path of a process log file. `stream` is "out" or "err". The name is
/// sanitized the same way as state files so it can never escape `log_dir`.
pub fn log_path(log_dir: &Path, name: &str, stream: &str) -> PathBuf {
    log_dir.join(format!("{}.{}.log", sanitize_process_name(name), stream))
}

pub struct LogHandler;

impl LogHandler {
    pub fn clear_logs(prefix: &str, log_dir: PathBuf) {
        for stream in ["out", "err"] {
            let path = log_path(&log_dir, prefix, stream);
            let _ = std::fs::remove_file(&path);
            // Also remove rotation files
            let _ = std::fs::remove_file(path.with_extension("log.bak"));
        }
    }

    pub fn stream_stdout(stdout: ChildStdout, prefix: String, log_dir: PathBuf) -> JoinHandle<()> {
        let log_file_path = log_path(&log_dir, &prefix, "out");
        let prefix = Arc::new(prefix);

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
        })
    }

    pub fn stream_stderr(stderr: ChildStderr, prefix: String, log_dir: PathBuf) -> JoinHandle<()> {
        let log_file_path = log_path(&log_dir, &prefix, "err");
        let prefix = Arc::new(prefix);

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
        })
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

    /// Print a log file and keep following it. With `start_lines`, only the
    /// last that many existing lines are printed first.
    pub async fn tail_logs(path: PathBuf, start_lines: Option<usize>) -> Result<()> {
        if !path.exists() {
            println!("Log file not found: {:?}", path);
            return Ok(());
        }

        // Print existing content, then follow. `pos` advances by exactly the
        // bytes printed so data appended mid-read is never printed twice.
        let mut pos = 0u64;
        if let Some(n) = start_lines {
            let bytes = tokio::fs::read(&path).await?;
            print!("{}", last_lines(&String::from_utf8_lossy(&bytes), n));
            pos = bytes.len() as u64;
        }
        loop {
            // A missing file means it is mid-rotation; retry on the next tick.
            if let Ok(metadata) = tokio::fs::metadata(&path).await {
                let len = metadata.len();
                if len < pos {
                    // File truncated or rotated
                    pos = 0;
                }
                if len > pos {
                    let mut file = File::open(&path).await?;
                    file.seek(std::io::SeekFrom::Start(pos)).await?;
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf).await?;
                    pos += buf.len() as u64;
                    print!("{}", String::from_utf8_lossy(&buf));
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
            }

            sleep(Duration::from_millis(500)).await;
        }
    }
}

/// The last `n` lines of `content`, including the trailing newline if any.
pub fn last_lines(content: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    let body = content.strip_suffix('\n').unwrap_or(content);
    let mut seen = 0;
    for (i, byte) in body.bytes().enumerate().rev() {
        if byte == b'\n' {
            seen += 1;
            if seen == n {
                return &content[i + 1..];
            }
        }
    }
    content
}

#[cfg(test)]
mod tests {
    use super::last_lines;

    #[test]
    fn takes_last_lines() {
        assert_eq!(last_lines("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(last_lines("a\nb\nc", 1), "c");
        assert_eq!(last_lines("a\nb\n", 10), "a\nb\n");
        assert_eq!(last_lines("a\nb\n", 0), "");
        assert_eq!(last_lines("", 3), "");
    }
}
