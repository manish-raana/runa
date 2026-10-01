<div align="center">

<pre>
 ____
|  _ \    _   _    _ __      __ _
| |_) |  | | | |  | '_ \    / _` |
|  _ <   | |_| |  | | | |  | (_| |
|_| \_\   \__,_|  |_| |_|   \__,_|
</pre>

# Runa

**A tiny, fast process supervisor for your terminal.**

Run any command, keep it alive, read its logs, and see which ports it's using, all from one small Rust binary.<br>
Think PM2, without the Node.js runtime or the config files.

[![CI](https://github.com/manish-raana/runa/actions/workflows/ci.yml/badge.svg)](https://github.com/manish-raana/runa/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)

[Install](#install) · [Quick start](#quick-start) · [Commands](#commands) · [Port dashboard](#port-dashboard) · [Metrics](#metrics) · [Menubar app](#macos-menubar-app) · [How it works](#how-it-works)

</div>

---

```console
$ runa run --name api --cmd "node server.js" --detach
Process 'api' started in background.

$ runa status
NAME                 PID        STATUS     PORT            STARTED              CMD
api                  86910      Running    3000            2026-09-29 10:40:47  node server.js
worker               87102      Running    -               2026-09-29 10:41:02  python3 worker.py
```

## Why Runa?

- **Works with any language.** Node, Bun, Python, Go, Rust, shell scripts: if you can type it, Runa can supervise it.
- **No config files.** One command starts a process. There's no ecosystem file or daemon to set up first.
- **Restarts crashed processes.** Choose `always`, `on-failure` or `never`. Retries back off exponentially, so a crash loop doesn't peg your CPU.
- **Shuts down cleanly.** It sends `SIGTERM` to the whole process group, waits up to 5 seconds, then sends `SIGKILL`. Grandchild processes don't get left behind.
- **Shows your ports.** `runa status` lists the ports each process is listening on. `runa watch` opens a live dashboard of every listening port on your machine.
- **Tracks CPU and memory.** Each process's CPU, memory and crash history is sampled in the background. `runa metrics` shows the last hour.
- **Knows your projects.** Every listening port is tagged with its project folder and git branch, even for servers Runa didn't start.
- **Keeps timestamped logs.** stdout and stderr are saved separately, rotate at 10 MB, and can be followed live.
- **Lives in your menubar.** On macOS, the optional Runa app shows your processes and dev servers one click away.
- **One small binary.** No runtime to install and no background daemon. Each process gets its own lightweight supervisor.

## Install

Runa runs on **macOS and Linux** (Intel and ARM). The command is `runa`.

```bash
# Homebrew
brew install manish-raana/tap/runa

# Shell installer (prebuilt binary)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/manish-raana/runa/releases/latest/download/runa-cli-installer.sh | sh

# Cargo (the crate is named runa-cli; it installs the `runa` command)
cargo install runa-cli
```

To build from source, you need Rust **1.88 or newer**:

```bash
git clone https://github.com/manish-raana/runa
cd runa
cargo install --path .
```

Check it worked with `runa --version`.

## Quick start

```bash
# Start a process in the background
runa run --name api --cmd "bun run index.ts" --detach

# See what's running, and on which ports
runa status

# Follow its logs
runa logs api --follow

# Restart it gracefully
runa restart api

# Stop it
runa stop api
```

Leave out `--detach` to run in the foreground. Output then streams to your terminal, and <kbd>Ctrl</kbd>+<kbd>C</kbd> shuts the process down cleanly.

### Scripting with `--json`

`runa status --json` prints an array with one object per process, for use in scripts and tools:

```console
$ runa status --json | jq '.[] | {name, status, ports, restarts}'
{
  "name": "api",
  "status": "running",
  "ports": [3000],
  "restarts": 0
}
```

Each object has these fields: `name`, `running`, `status` (`running`, `failed` or `dead`), `pid`, `child_pid`, `cmd`, `cwd`, `ports`, `restarts`, `last_exit_code`, `last_error`, `cpu` (percent of one core), `rss_kb`, `project` (see [Ports and projects](#ports-and-projects)) and `started_at` (RFC 3339). `cpu` and `rss_kb` cover the whole process group and are `null` until the first sample, a few seconds after start. When nothing is tracked, the output is `[]`.

## Project files (`runa.toml`)

Put a `runa.toml` in your project and start the whole stack with one command:

```toml
[processes.api]
cmd = "bun run index.ts"
cwd = "./api"
env = { PORT = 3000 }
env_file = ".env"

[processes.worker]
cmd = "python3 worker.py"
restart = "on-failure"
```

```console
$ runa up
  api     started (PID 4120)
  worker  started (PID 4123)

$ runa down
  api     stopped
  worker  stopped
```

- `runa up` starts every process in the file in the background. Processes that are already running are skipped. If any process fails to start, `runa up` says why and exits with an error.
- `runa down` stops the processes in the file and waits until they have exited. Runa processes that aren't in the file are left alone.
- To act on some processes only, name them: `runa up api`, `runa down worker`.
- `runa init` creates a commented starter `runa.toml`.
- Use `-f path/to/runa.toml` to point at a different file.

| Field | Default | Description |
| --- | --- | --- |
| `cmd` | *required* | The command to run. It's split with shell-style quoting, like `--cmd`. |
| `cwd` | the file's directory | Working directory, relative to `runa.toml` |
| `restart` | `always` | `always`, `on-failure` or `never` |
| `env` | none | Extra environment variables. Numbers and booleans are allowed. |
| `env_file` | none | A dotenv file to load, relative to `runa.toml` |

Each process gets your shell's environment, then the `env_file` variables, then the `env` table. Later sources win when a name repeats. These values are passed to the process through its environment, never on its command line, so they don't show up in `ps`.

> [!NOTE]
> Process names are global on your machine. If another project already has a process running under the same name, `runa up` reports the conflict instead of starting a second copy, and `runa down` won't stop the other project's process.

## Commands

| Command | What it does |
| --- | --- |
| `runa run --name <name> --cmd "<command>"` | Start and supervise a process |
| `runa up [names…]` | Start the processes in `runa.toml` |
| `runa down [names…]` | Stop the processes in `runa.toml` and wait for them to exit |
| `runa init` | Create a starter `runa.toml` |
| `runa status [--json]` | List tracked processes with PID, status, restarts, ports and start time |
| `runa logs <name> [-n N] [--follow]` | Print a process's logs (or the last N lines), or follow them live |
| `runa restart <name>` | Gracefully restart the process |
| `runa stop <name>` | Gracefully stop the process |
| `runa stop --all` | Stop every tracked process |
| `runa flush <name>` | Delete a process's log files |
| `runa ports [--all] [--udp] [--json]` | List the ports of runa processes and dev servers. `--all` adds system services, app helpers and random high ports. |
| `runa metrics <name> [--since SECS] [--json]` | Show CPU, memory and start/exit history for a process |
| `runa watch` | Open the live port dashboard |
| `runa save` | Remember the running processes |
| `runa resurrect` | Start the saved processes again |
| `runa startup [--remove]` | Run `resurrect` automatically at login |
| `runa completions <shell>` | Print a shell completion script |
| `runa mcp` | Run an MCP server for AI agents (see [Use with AI agents](#use-with-ai-agents)) |

Run `runa <command> --help` for every option.

### `runa run` options

| Flag | Default | Description |
| --- | --- | --- |
| `--name <name>` | *required* | A unique name for the process |
| `--cmd "<command>"` | *required* | The command to run. It's split with shell-style quoting, but it is **not** run through a shell (see below). |
| `--restart <policy>` | `always` | `always`, `on-failure` or `never` |
| `-e, --env KEY=VALUE` | none | Set an environment variable. Repeat the flag for more. |
| `-d, --detach` | off | Run the supervisor in the background |

```bash
runa run --name worker \
  --cmd "python3 worker.py --queue high" \
  --restart on-failure \
  -e REDIS_URL=redis://localhost:6379 \
  -e DEBUG=true \
  --detach
```

> [!TIP]
> Commands aren't run through a shell, so pipes, `&&`, globs and `$VARS` aren't interpreted. When you need them, wrap the command in a shell:
> ```bash
> runa run --name build --cmd "sh -c 'npm run build && npm start'"
> ```

### Restart policies

| Policy | Restarts when… |
| --- | --- |
| `always` | the process exits for any reason |
| `on-failure` | the process exits with a non-zero code |
| `never` | never. Runa stops supervising once the process exits. |

Restarts back off exponentially: the wait starts at **1s** and doubles each time, up to **30s**. If a process ran for at least 30 seconds before exiting, the wait resets to 1s. A process that crashes once a day restarts right away; one that crashes on startup doesn't spin.

## Surviving reboots

Save the processes you have running, and Runa can start them again after a restart:

```bash
runa save        # remember what's running now
runa startup     # start the saved processes at every login
```

- **`runa save`** records each running process: its command, working directory, restart policy and `-e` variables. Run it again whenever your set of processes changes. Processes started from a `runa.toml` are restored from that file, so `env_file` values are read fresh rather than copied into the snapshot.
- **`runa resurrect`** starts every saved process that isn't already running. You can run it by hand at any time.
- **`runa startup`** installs a login item that runs `runa resurrect`: a launchd agent on macOS (`~/Library/LaunchAgents/dev.runa.resurrect.plist`) or a systemd user service on Linux (`runa-resurrect.service`). It records your current `PATH`, so commands like `npm` are found the same way they are in your shell. `runa startup --remove` uninstalls it.

On Linux, user services start when you log in. To start them at boot without logging in, also run `loginctl enable-linger`.

## Shell completions

```bash
# zsh
runa completions zsh > "${fpath[1]}/_runa"

# bash
runa completions bash > ~/.local/share/bash-completion/completions/runa

# fish
runa completions fish > ~/.config/fish/completions/runa.fish
```

Start a new shell afterwards. `elvish` and `powershell` are supported too.

## Port dashboard

`runa watch` opens a live terminal dashboard of every listening port on your machine. Ports owned by Runa processes are highlighted, and conflicts are flagged. Use it to answer "what's already on port 3000?".

```bash
runa watch                 # TCP listeners
runa watch --udp           # include UDP sockets
runa watch --filter api    # start with a filter applied
runa watch --interval 500  # refresh every 500ms (minimum 250ms)
```

- **Green** rows belong to a Runa process.
- **Red** rows are conflicts: more than one process on the same port.
- The side panel shows details, or the last lines of the logs for Runa processes.

| Key | Action |
| --- | --- |
| <kbd>↑</kbd> <kbd>↓</kbd> / <kbd>k</kbd> <kbd>j</kbd> | Move the selection |
| <kbd>Home</kbd> / <kbd>End</kbd> | Jump to the first or last row |
| <kbd>/</kbd> | Filter by port, PID, command, user or Runa name |
| <kbd>t</kbd> | Change the sort: port, PID, command, Runa or conflict |
| <kbd>u</kbd> | Show or hide UDP sockets |
| <kbd>o</kbd> | Switch the side panel between details and logs |
| <kbd>c</kbd> | Copy the selected row's details to the clipboard |
| <kbd>s</kbd> | Stop the selected Runa process (other processes get `SIGTERM`) |
| <kbd>R</kbd> | Restart the selected Runa process |
| <kbd>K</kbd> | Send `SIGTERM` to the selected PID |
| <kbd>r</kbd> | Refresh now |
| <kbd>q</kbd> | Quit |

The mouse works too: click a row to select it, scroll to move, and click the action buttons. Every destructive action asks for confirmation first (<kbd>y</kbd>/<kbd>Enter</kbd> to confirm, <kbd>n</kbd>/<kbd>Esc</kbd> to cancel).

### Ports and projects

`runa ports` lists the ports of Runa processes and of dev servers you started yourself, tagged with their project:

```console
$ runa ports
PROTO  PORT    ADDRESS          PID      COMMAND              KIND       PROJECT                  RUNA
TCP    3000    0.0.0.0          46127    node                 dev        shop/web@main            -
TCP    8000    127.0.0.1        51230    Python               runa       api@feature/auth         api
```

In `runa ports --json`, each port has a `kind`: `runa`, `dev` (something you started: a dev server, database or container port), `ephemeral` (a helper on a random port at 49152 or above), `app` (inside a GUI `.app` bundle) or `system` (a macOS service, or a `root` or `_service` user's process). Without `--all`, only `runa` and `dev` ports are listed.

Each port and each process in `runa status --json` also has a `project`, detected from the process's working directory, even for servers runa didn't start: `{name, sub, root, branch}`. `name` is the project folder (the nearest git root), `sub` is the monorepo package the process runs in (named from its `package.json`, `Cargo.toml`, `pyproject.toml` or `go.mod`), and `branch` is read from `.git/HEAD`. Processes running from `/`, your home folder, app bundles or hidden tool folders get no project.

## Metrics

Each supervisor samples its process group's CPU and memory every 5 seconds and records lifecycle events: starts, exits (with the exit code), restarts and stops.

```console
$ runa metrics api
120 samples over 595s
CPU     now    2.1%   avg    3.4%   peak   41.0%
Memory  now 182.3 MB   avg 176.0 MB   peak 201.5 MB
Procs   3

Events:
  2026-10-01 13:13:21  start    pid 33518
  2026-10-01 13:18:02  exit     exit code 1
  2026-10-01 13:18:03  start    pid 33702
```

- History is kept in `~/.runa/metrics/<name>.samples.jsonl` and `<name>.events.jsonl`: the last hour of samples and the last 200 events. Both are cleared when the process is started with `runa run`, like its logs.
- `--json` prints `{name, interval_secs, samples: [{t, cpu, rss_kb, procs}], events: [{t, event, pid?, code?}]}`, with `t` in Unix seconds.

## macOS menubar app

`macos/` contains **Runa**, a native menubar app for macOS 14 and later. On macOS 26 it uses Liquid Glass.

- **At a glance:** the menubar icon shows how many processes are running and how many dev ports are listening. A dot appears on the icon when a process is down.
- **Processes:** status, project and branch, ports, CPU and memory. Click a port to open it in the browser, hover to restart or stop, and right-click to reveal the project in Finder.
- **Details:** CPU and memory charts for the last hour, an activity timeline of starts, exits and restarts, and recent logs.
- **Dev servers:** other servers listening on your machine, tagged with their project, with a button to stop them. **Show all** adds system services and app helpers.

```bash
macos/scripts/bundle.sh          # builds macos/build/Runa.app
open macos/build/Runa.app
```

Building it needs Xcode or the Command Line Tools. The app drives the `runa` CLI (0.4.0 or later), which it looks for in `/opt/homebrew/bin`, `/usr/local/bin`, `~/.cargo/bin` and your shell's `PATH`. To start it at login, add `Runa.app` under **System Settings › General › Login Items**.

## Use with AI agents

Coding agents can use Runa to start dev servers without blocking their shell, check which ports are up, and read logs. There are two ways to connect them, and they work well together.

### MCP server

`runa mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io) server over stdio. It gives agents typed tools instead of CLI output to parse:

| Tool | What it does |
| --- | --- |
| `list_processes` | Every process with its status, ports, restarts and last error, as JSON |
| `get_logs` | The last lines of a process's stdout and stderr |
| `start_process` | Start a command in the background and report whether it came up |
| `stop_process` | Stop a process and wait until it has exited |
| `restart_process` | Restart a process gracefully |
| `project_up` / `project_down` | Start or stop the processes in a `runa.toml` |

Add it to Claude Code:

```bash
claude mcp add --scope user runa -- runa mcp
```

For other MCP clients, add this to their server configuration:

```json
{
  "mcpServers": {
    "runa": { "command": "runa", "args": ["mcp"] }
  }
}
```

The server runs in the directory the client starts it in, which is usually your project, so relative `cwd` values and `project_up` without a `file` resolve from there.

### Agent skill

This repo also includes a ready-made skill, [skills/runa/SKILL.md](skills/runa/SKILL.md), that teaches an agent how to use Runa well: always start in the background, never run `logs -f` or `watch`, verify a server before relying on it, and clean up afterwards. It works with or without the MCP server.

Install it for Claude Code (other agents that support [Agent Skills](https://agentskills.io) use the same folder format, in their own skills directory):

```bash
# For every project
mkdir -p ~/.claude/skills/runa
curl -fsSL https://raw.githubusercontent.com/manish-raana/runa/main/skills/runa/SKILL.md \
  -o ~/.claude/skills/runa/SKILL.md

# Or for one project only: use <project>/.claude/skills/runa/ instead
```

The agent also needs the `runa` binary on its `PATH`. `runa status --json` and `runa logs <name> -n 50` are the two commands built for agents: both return immediately with output that's easy to parse.

## Logs

Each process writes two log files, and every line is timestamped:

```
~/.runa/<name>.out.log    # stdout
~/.runa/<name>.err.log    # stderr
```

```text
[2026-09-29 10:40:47] [api] Server listening on :3000
```

- `runa logs <name>` prints both files. `-n 50` prints only the last 50 lines of each. `--follow` keeps streaming stdout and stderr as they're written; with `-n`, it starts from the last N lines.
- When a log file passes **10 MB**, it's renamed to `*.log.bak`, and a new file is started.
- Logs are **cleared each time a process is started** with `runa run`. Restarts caused by crashes or `runa restart` keep the existing logs.

## How it works

```
runa run --name api --cmd "node server.js"
   │
   └─► supervisor (one per process)          ~/.runa/api.json   ← PID, status, restart count
          │
          └─► node server.js                 own process group
                 ├── stdout ──► ~/.runa/api.out.log
                 └── stderr ──► ~/.runa/api.err.log
```

- **One supervisor per process.** Each `runa run` starts its own supervisor. With `--detach`, the supervisor moves to a new session in the background. There's no central daemon that can crash and take everything down with it.
- **State lives in plain files.** Each process's state is a small JSON file in `~/.runa/`. Other commands read these files, then signal the supervisor: `stop` sends `SIGTERM` and `restart` sends `SIGHUP`.
- **Each child gets its own process group.** Shutdown signals go to the whole group, so workers and subprocesses the child starts are stopped too.
- **Stale state files are detected.** Before signalling a PID, Runa checks that it still belongs to a Runa supervisor. If the OS has reused the PID for another program, that program is left alone.
- **Port detection** uses `lsof`, and falls back to `ss` on Linux. Ports are matched to Runa processes by PID and by the child's process group.

## Limitations

Runa is deliberately small. It does **not** yet:

- run clusters or multiple instances of one process
- watch files and reload on changes
- run on Windows

If you need one of these, please open an issue.

## Development

```bash
cargo build           # debug build
cargo test            # run the test suite
cargo clippy          # lint
cargo run -- status   # run the CLI from source
```

Contributions are welcome. Please open an issue to discuss larger changes before sending a pull request.

## License

[MIT](LICENSE)
