---
name: runa
description: Run and manage long-running processes (dev servers, workers, watchers, databases) in the background with the `runa` CLI, without blocking the terminal. Use when you need to start a server and keep working, check whether something is running or which port it listens on, read a background process's logs, restart it after code changes, stop what you started, start/stop a project's stack from runa.toml, or find out why a service is crashing or what is using a port.
---

# Runa

Runa is a process supervisor. It starts a command in the background, restarts
it if it crashes, keeps its stdout/stderr in log files, and shows the ports it
listens on. Use it instead of `cmd &`, `nohup` or a blocking foreground
command, so your shell stays free and the process can still be inspected.

## Setup

```bash
command -v runa || echo "runa is not installed"
```

If it's missing, tell the user and suggest `brew install manish-raana/tap/runa`
or `cargo install runa-cli`. Don't install software without the user's
permission.

**If Runa's MCP tools are available** (`list_processes`, `get_logs`,
`start_process`, `stop_process`, `restart_process`, `project_up`,
`project_down`), prefer them over the CLI: they return structured results and
never block. The rules and workflows below still apply; each CLI command maps
to the tool of the same purpose. The user can enable them with
`claude mcp add --scope user runa -- runa mcp`.

## Rules

1. **Always start in the background.** Use `runa run ... --detach` or
   `runa up`. A plain `runa run` without `--detach` blocks until the process
   exits.
2. **Never run commands that don't return.** `runa logs -f` follows forever,
   and `runa watch` is an interactive TUI. Use `runa logs <name> -n 50` and
   `runa status --json` instead.
3. **Check before you start.** Run `runa status --json` first. If a process
   for this job is already running, reuse or restart it instead of starting a
   duplicate.
4. **Use descriptive, project-specific names** (`shop-api`, `shop-web`).
   Names are global on the machine, not per project.
5. **Verify, don't assume.** A successful start only means the process
   launched. Confirm it's healthy with `status --json` (running, port) and
   the logs.
6. **Clean up.** When your task is done, stop the processes you started,
   unless the user wants them kept running. Tell the user what you left
   running.
7. **Don't kill processes Runa doesn't manage** (for example, whatever holds
   a port you need) without asking the user.

## Start a server and verify it

```bash
runa run --name shop-api --cmd "npm run dev" --detach
```

`run --detach` exits non-zero if the command can't be started (for example, a
missing binary). Then wait until it's actually serving, up to 30s. If you know
the port:

```bash
for i in $(seq 1 30); do curl -s -o /dev/null http://localhost:3000/ && break; sleep 1; done
```

If you don't know the port, wait until Runa sees the process listening
(needs `jq`):

```bash
for i in $(seq 1 30); do
  runa status --json | jq -e '.[] | select(.name=="shop-api") | .ports | length > 0' >/dev/null && break
  sleep 1
done
```

Then confirm it's healthy:

```bash
runa status --json        # running: true, the expected port in "ports"
runa logs shop-api -n 30  # no errors on startup
```

If the process runs but never shows a port, check its logs. It may be
listening on a different port, or still compiling.

## Projects with a `runa.toml`

If the project root has a `runa.toml`, prefer it over individual `run`
commands:

```bash
runa up              # start every process in the file (skips running ones)
runa up api          # start just one
runa down            # stop them and wait until they have exited
runa down worker
```

`runa up` prints one line per process and exits non-zero if any failed to
start. File format:

```toml
[processes.api]
cmd = "npm run dev"
cwd = "./api"            # optional, relative to runa.toml
restart = "always"       # always | on-failure | never
env = { PORT = 3000 }    # optional
env_file = ".env"        # optional
```

If a project needs the same processes again and again, offer to create a
`runa.toml` (`runa init` writes a template).

## After changing code

```bash
runa restart shop-api     # graceful restart; logs are kept
runa logs shop-api -n 30
```

Skip this if the command already reloads on file changes (for example
`npm run dev` with a watcher).

## Debugging a crashing process

```bash
runa status --json   # look at status, restarts, last_exit_code, last_error
runa logs shop-api -n 100
```

- `status: "failed"` with `last_error`: the command couldn't be started.
  Usually it's a wrong binary name or path, or a missing dependency.
- `restarts` keeps growing: the process starts, then crashes. Read the
  stderr section of the logs. Runa waits longer between restarts each time,
  up to 30s.
- `status: "dead"`: the supervisor is gone. Start the process again.

## "Port already in use"

```bash
runa status --json                          # is a Runa process holding it?
lsof -nP -iTCP:3000 -sTCP:LISTEN            # who else is holding it?
```

If a Runa process holds the port, stop or restart it by name. If something
else holds it, tell the user what it is (command and PID) and ask before
killing it.

## Command reference

| Command | Notes |
| --- | --- |
| `runa run --name N --cmd "C" --detach` | Options: `--restart always\|on-failure\|never`, `-e KEY=VALUE` (repeatable) |
| `runa status --json` | JSON array: `name`, `running`, `status`, `pid`, `child_pid`, `cmd`, `cwd`, `ports`, `restarts`, `last_exit_code`, `last_error`, `started_at` |
| `runa logs N -n 50` | Last 50 lines of stdout and stderr; returns immediately |
| `runa restart N` | Graceful restart; errors if not running |
| `runa stop N` / `runa stop --all` | Sends the stop signal and returns without waiting (shutdown can take several seconds) |
| `runa up [names]` / `runa down [names]` | Start or stop `runa.toml` processes; `down` waits for exit |
| `runa flush N` | Delete a process's logs |

## Pitfalls

- **`--cmd` is not run through a shell.** Pipes, `&&`, `cd`, globs and
  `$VARS` are not interpreted. Wrap them instead:
  `--cmd "sh -c 'cd api && npm start'"`.
- **Logs are wiped** each time a process is started with `run` or `up`.
  They're kept across restarts and crashes. Read them before starting again
  if you need the old output.
- **`-e` values appear in `ps` output**, because they're command-line
  arguments. Put secrets in an `env_file` in `runa.toml` instead.
- **The working directory** is where you run `runa run`. With `runa.toml`,
  it's `cwd`, relative to the file.
- **After `runa stop`,** wait a few seconds, or check `runa status --json`,
  before reusing the port or name. `runa down` already waits.
