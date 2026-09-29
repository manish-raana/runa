//! `runa startup`: run `runa resurrect` at login, via a launchd agent on
//! macOS or a systemd user service on Linux.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const LAUNCHD_LABEL: &str = "dev.runa.resurrect";
const SYSTEMD_UNIT: &str = "runa-resurrect.service";

/// Install the login item. Returns a message describing what was done.
pub fn install() -> Result<String> {
    let exe = runa_exe()?;
    // launchd and systemd start jobs with a minimal PATH; capture the user's
    // so commands like `npm` or `bun` resolve the same way they do now.
    let path_env = std::env::var("PATH").unwrap_or_default();

    if cfg!(target_os = "macos") {
        let file = launchd_plist_path()?;
        let log = home()?.join(".runa").join("startup.log");
        write_file(&file, &launchd_plist(&exe, &path_env, &log))?;
        Ok(format!(
            "Installed {}.\nSaved processes will start at your next login (log: {}).",
            file.display(),
            log.display()
        ))
    } else if cfg!(target_os = "linux") {
        let file = systemd_unit_path()?;
        write_file(&file, &systemd_unit(&exe, &path_env))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", SYSTEMD_UNIT])?;
        Ok(format!(
            "Installed and enabled {}.\nSaved processes will start at your next login. \
             To also start them at boot without logging in, run `loginctl enable-linger`.",
            file.display()
        ))
    } else {
        anyhow::bail!("`runa startup` supports macOS and Linux only")
    }
}

/// Remove the login item. Returns a message describing what was done.
pub fn uninstall() -> Result<String> {
    if cfg!(target_os = "macos") {
        let file = launchd_plist_path()?;
        remove_file(&file)
    } else if cfg!(target_os = "linux") {
        let file = systemd_unit_path()?;
        if file.exists() {
            // Best effort: the unit may never have been enabled.
            let _ = systemctl(&["disable", SYSTEMD_UNIT]);
        }
        let message = remove_file(&file)?;
        let _ = systemctl(&["daemon-reload"]);
        Ok(message)
    } else {
        anyhow::bail!("`runa startup` supports macOS and Linux only")
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME not set")
}

fn launchd_plist_path() -> Result<PathBuf> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

fn systemd_unit_path() -> Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home()?.join(".config"),
    };
    Ok(config.join("systemd/user").join(SYSTEMD_UNIT))
}

/// The `runa` to run at login: the one on PATH when there is one (a stable
/// path such as /opt/homebrew/bin/runa), else this executable.
fn runa_exe() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("runa");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    std::env::current_exe().context("Failed to get current executable path")
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

fn remove_file(path: &Path) -> Result<String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(format!("Removed {}.", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok("Startup was not installed.".to_string())
        }
        Err(err) => Err(err).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

fn systemctl(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("Failed to run systemctl")?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn launchd_plist(exe: &Path, path_env: &str, log: &Path) -> String {
    let exe = xml_escape(&exe.display().to_string());
    let path_env = xml_escape(path_env);
    let log = xml_escape(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>resurrect</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- Don't kill the supervisors this job starts when it exits. -->
    <key>AbandonProcessGroup</key>
    <true/>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path_env}</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

fn systemd_unit(exe: &Path, path_env: &str) -> String {
    let exe = systemd_quote(&exe.display().to_string());
    let path_env = systemd_quote(&format!("PATH={path_env}"));
    format!(
        "[Unit]
Description=Start processes saved with `runa save`

[Service]
Type=oneshot
RemainAfterExit=yes
# Keep the supervisors running after `runa resurrect` exits.
KillMode=process
Environment={path_env}
ExecStart={exe} resurrect

[Install]
WantedBy=default.target
"
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Quote a value for a systemd unit file; `%` starts a specifier there.
fn systemd_quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_values() {
        let plist = launchd_plist(
            Path::new("/opt/a&b/runa"),
            "/usr/bin:/x<y>",
            Path::new("/h/.runa/startup.log"),
        );
        assert!(plist.contains("<string>/opt/a&amp;b/runa</string>"));
        assert!(plist.contains("<string>/usr/bin:/x&lt;y&gt;</string>"));
        assert!(plist.contains("<string>resurrect</string>"));
        assert!(plist.contains(LAUNCHD_LABEL));
    }

    #[test]
    fn systemd_unit_quotes_values() {
        let unit = systemd_unit(Path::new("/home/me/my bin/runa"), "/usr/bin:/50%");
        assert!(unit.contains("ExecStart=\"/home/me/my bin/runa\" resurrect"));
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/50%%\""));
        assert!(unit.contains("WantedBy=default.target"));
    }
}
