<div align="center">

# Runa

**A tiny, fast process supervisor for your terminal.**

Run any command, keep it alive, read its logs, and see which ports it's using, all from one small Rust binary.<br>
Think PM2, without the Node.js runtime or the config files.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)

[Install](#install) · [Quick start](#quick-start) · [Commands](#commands) · [Port dashboard](#port-dashboard) · [How it works](#how-it-works)

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
- **Keeps timestamped logs.** stdout and stderr are saved separately, rotate at 10 MB, and can be followed live.
- **One small binary.** No runtime to install and no background daemon. Each process gets its own lightweight supervisor.

## Install

Runa runs on **macOS and Linux**. Building it needs Rust **1.88 or newer**.

```bash
# Install straight from GitHub
cargo install --git https://github.com/manish-raana/runa

# …or from a local clone
git clone https://github.com/manish-raana/runa
cd runa
cargo install --path .
```

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

## Commands

| Command | What it does |
| --- | --- |
| `runa run --name <name> --cmd "<command>"` | Start and supervise a process |
| `runa status` | List tracked processes with PID, status, ports and start time |
| `runa logs <name> [--follow]` | Print a process's logs, or follow them live |
| `runa restart <name>` | Gracefully restart the process |
| `runa stop <name>` | Gracefully stop the process |
| `runa stop --all` | Stop every tracked process |
| `runa flush <name>` | Delete a process's log files |
| `runa watch` | Open the live port dashboard |

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

## Logs

Each process writes two log files, and every line is timestamped:

```
~/.runa/<name>.out.log    # stdout
~/.runa/<name>.err.log    # stderr
```

```text
[2026-09-29 10:40:47] [api] Server listening on :3000
```

- `runa logs <name>` prints both files. `--follow` keeps streaming stdout and stderr as they're written.
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

- start processes again after a reboot (no systemd or launchd integration)
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
