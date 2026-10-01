//! Names the project a process belongs to from its working directory, so
//! listeners runa didn't start can still be tagged "my-app · main".

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The project's folder name: what people call it, where manifest
    /// names are often generic (`my-app`, `frontend`).
    pub name: String,
    /// The package inside a monorepo the process runs in, e.g. `web`.
    pub sub: Option<String>,
    /// The project's root directory.
    pub root: String,
    /// The checked-out git branch, or a short commit hash when detached.
    pub branch: Option<String>,
}

/// Files that mark a package root, in the order their names are preferred.
const MANIFESTS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "go.mod",
    "deno.json",
    "composer.json",
    "Gemfile",
    "mix.exs",
];

/// Detects the project around `cwd`. `home` bounds the search, so a
/// dotfiles repo in the home folder never claims every process.
pub fn detect(cwd: &Path, home: &Path) -> Option<Workspace> {
    if !is_candidate(cwd, home) {
        return None;
    }

    let mut package: Option<PathBuf> = None;
    let mut repo: Option<PathBuf> = None;
    for dir in cwd.ancestors() {
        if dir == home || dir.parent().is_none() {
            break;
        }
        if package.is_none() && MANIFESTS.iter().any(|m| dir.join(m).is_file()) {
            package = Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            repo = Some(dir.to_path_buf());
            break;
        }
    }

    let (root, sub) = match (repo, package) {
        (Some(repo), Some(package)) if package != repo => {
            let sub = package_name(&package).unwrap_or_else(|| folder_name(&package));
            (repo, Some(strip_scope(&sub)))
        }
        (Some(repo), _) => (repo, None),
        (None, Some(package)) => (package, None),
        // No markers: a folder under home is still a useful hint.
        (None, None) if cwd.starts_with(home) => (cwd.to_path_buf(), None),
        (None, None) => return None,
    };

    Some(Workspace {
        name: folder_name(&root),
        sub,
        branch: git_branch(&root),
        root: root.display().to_string(),
    })
}

/// Skips directories that say nothing about a project: `/`, home itself,
/// app bundles, system folders and hidden tool folders like `~/.vscode`.
fn is_candidate(cwd: &Path, home: &Path) -> bool {
    if cwd == home || cwd.parent().is_none() {
        return false;
    }
    let path = cwd.to_string_lossy();
    if path.contains(".app/") || path.ends_with(".app") {
        return false;
    }
    if let Ok(rest) = cwd.strip_prefix(home) {
        // ~/Library and hidden folders (~/.vscode/extensions/...).
        let first = rest.components().next();
        return !first.is_some_and(|c| {
            let c = c.as_os_str().to_string_lossy();
            c.starts_with('.') || c == "Library"
        });
    }
    const SYSTEM: &[&str] = &[
        "/System", "/Library", "/usr", "/bin", "/sbin", "/private", "/opt", "/var", "/etc",
    ];
    !SYSTEM.iter().any(|p| cwd.starts_with(p))
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

fn strip_scope(name: &str) -> String {
    match name.strip_prefix('@').and_then(|n| n.split_once('/')) {
        Some((_, bare)) => bare.to_string(),
        None => name.to_string(),
    }
}

/// The name a package gives itself in its manifest, used for monorepo
/// packages, whose folders are often just `web` or `api`.
fn package_name(dir: &Path) -> Option<String> {
    let read = |file: &str| fs::read_to_string(dir.join(file)).ok();

    if let Some(text) = read("package.json") {
        let json: serde_json::Value = serde_json::from_str(&text).ok()?;
        if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
            return Some(name.to_string());
        }
    }
    for (file, tables) in [
        ("Cargo.toml", &["package"][..]),
        ("pyproject.toml", &["project", "tool.poetry"][..]),
    ] {
        let Some(text) = read(file) else { continue };
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue;
        };
        for table in tables {
            let mut value = Some(&doc);
            for key in table.split('.') {
                value = value.and_then(|t| t.get(key)).and_then(|v| v.as_table());
            }
            if let Some(name) = value.and_then(|t| t.get("name")).and_then(|v| v.as_str()) {
                return Some(name.to_string());
            }
        }
    }
    if let Some(text) = read("go.mod") {
        let module = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("module "))?;
        return module.trim().rsplit('/').next().map(str::to_string);
    }
    None
}

/// Reads the branch from `.git/HEAD` without running git. Follows the
/// `gitdir:` pointer that worktrees and submodules use.
pub fn git_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let pointer = fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(pointer.trim().strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() {
            dir
        } else {
            root.join(dir)
        }
    } else {
        dot_git
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None => head.get(..7).map(str::to_string),
    }
}

/// Working directories of `pids`, where the OS reports them.
pub fn process_cwds(pids: &[i32]) -> HashMap<i32, PathBuf> {
    let mut cwds = HashMap::new();
    let mut missing = Vec::new();
    for &pid in pids {
        match fs::read_link(format!("/proc/{pid}/cwd")) {
            Ok(path) => {
                cwds.insert(pid, path);
            }
            Err(_) => missing.push(pid.to_string()),
        }
    }
    if missing.is_empty() {
        return cwds;
    }
    // macOS: one lsof call for every PID. `-F pn` prints `p<pid>` then `n<path>`.
    let list = missing.join(",");
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-F", "pn", "-p", &list])
        .output()
    else {
        return cwds;
    };
    let mut pid = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), pid) {
            cwds.insert(pid, PathBuf::from(path));
        }
    }
    cwds
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempHome(PathBuf);

    impl TempHome {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("runa-ws-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_a_monorepo_package_and_branch() {
        let home = TempHome::new("mono");
        home.write("code/shop/package.json", r#"{"name": "shop"}"#);
        home.write("code/shop/.git/HEAD", "ref: refs/heads/feature/cart\n");
        home.write(
            "code/shop/apps/web/package.json",
            r#"{"name": "@shop/web"}"#,
        );
        let cwd = home.0.join("code/shop/apps/web/src");
        fs::create_dir_all(&cwd).unwrap();

        let ws = detect(&cwd, &home.0).unwrap();

        assert_eq!(ws.name, "shop");
        assert_eq!(ws.sub.as_deref(), Some("web"));
        assert_eq!(ws.branch.as_deref(), Some("feature/cart"));
        assert!(ws.root.ends_with("code/shop"));
    }

    #[test]
    fn names_projects_by_folder_and_packages_by_manifest() {
        let home = TempHome::new("names");
        home.write("mono/.git/HEAD", "ref: refs/heads/main\n");
        home.write("mono/package.json", r#"{"name": "generic-template"}"#);
        home.write(
            "mono/crates/a/Cargo.toml",
            "[package]\nname = \"rusty-api\"\n",
        );
        home.write(
            "mono/py/pyproject.toml",
            "[tool.poetry]\nname = \"poetic\"\n",
        );
        home.write(
            "mono/gosvc/go.mod",
            "module github.com/me/gosvc-mod\n\ngo 1.22\n",
        );
        home.write("mono/bare/deno.json", "{}");
        home.write("plain/notes.txt", "");

        let ws = |dir: &str| detect(&home.0.join(dir), &home.0).unwrap();

        assert_eq!(ws("mono").name, "mono");
        assert_eq!(ws("mono").sub, None);
        assert_eq!(ws("mono/crates/a").sub.as_deref(), Some("rusty-api"));
        assert_eq!(ws("mono/py").sub.as_deref(), Some("poetic"));
        assert_eq!(ws("mono/gosvc").sub.as_deref(), Some("gosvc-mod"));
        assert_eq!(ws("mono/bare").sub.as_deref(), Some("bare"));
        assert_eq!(ws("plain").name, "plain");
    }

    #[test]
    fn ignores_home_system_app_and_hidden_folders() {
        let home = TempHome::new("skip");
        // A dotfiles repo in home must not claim everything under it.
        home.write(".git/HEAD", "ref: refs/heads/main\n");
        home.write(
            ".vscode/extensions/pylance/package.json",
            r#"{"name": "pylance"}"#,
        );
        home.write("proj/index.js", "");

        assert_eq!(detect(&home.0, &home.0), None);
        assert_eq!(detect(Path::new("/"), &home.0), None);
        assert_eq!(
            detect(&home.0.join(".vscode/extensions/pylance"), &home.0),
            None
        );
        assert_eq!(
            detect(
                Path::new("/Applications/Openship.app/Contents/Resources"),
                &home.0
            ),
            None
        );
        assert_eq!(detect(Path::new("/usr/libexec"), &home.0), None);
        let proj = detect(&home.0.join("proj"), &home.0).unwrap();
        assert_eq!((proj.name.as_str(), proj.branch), ("proj", None));
    }

    #[test]
    fn reads_worktree_and_detached_heads() {
        let home = TempHome::new("git");
        home.write(
            "main-repo/.git/worktrees/wt/HEAD",
            "ref: refs/heads/hotfix\n",
        );
        let gitdir = home.0.join("main-repo/.git/worktrees/wt");
        home.write("wt/.git", &format!("gitdir: {}\n", gitdir.display()));
        home.write(
            "detached/.git/HEAD",
            "3f9c2a1b8e7d6c5b4a39281706f5e4d3c2b1a090\n",
        );

        assert_eq!(git_branch(&home.0.join("wt")).as_deref(), Some("hotfix"));
        assert_eq!(
            git_branch(&home.0.join("detached")).as_deref(),
            Some("3f9c2a1")
        );
    }

    #[test]
    fn strips_npm_scopes() {
        assert_eq!(strip_scope("@acme/web"), "web");
        assert_eq!(strip_scope("web"), "web");
    }
}
