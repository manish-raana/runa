# Runa

Runa is a fast, language-agnostic process runner and supervisor designed for local development and lightweight production use. It is a minimal, Rust-based alternative to tools like PM2, focusing on simplicity, correctness, and developer ergonomics.

## Features

- **Language Agnostic**: Run any command (Node, Python, Bun, Go, Rust, etc.).
- **Zero Configuration**: No complex setup required. Simple, predictable CLI.
- **Detached Mode**: Run supervisors in the background natively with `--detach`.
- **Automatic Port Detection**: See which ports your processes are listening on in the status table.
- **Graceful Shutdown**: Sends `SIGTERM` to the entire process group, followed by a timeout before `SIGKILL`.
- **Automatic Restarts**: Configurable restart policies (`always`, `on-failure`, `never`).
- **Professional Logging**: 
    - Real-time log following with `--follow`.
    - Timestamps on every log line.
    - Automatic log rotation (10MB threshold).
    - Manual log clearing with `flush`.
- **Single Binary**: Distributed as a single static binary.

## Installation

```bash
cargo build --release
cargo install --path .
```

## Usage

### Start a process
```bash
# General
runa run --name api --cmd "node server.js" --restart always

# Run in background (Detached)
runa run --name my-api --cmd "bun run index.ts" --detach
```

### Start with environment variables
```bash
runa run --name worker --cmd "python3 worker.py" -e REDIS_URL=localhost:6379 -e DEBUG=true
```

### List running processes (with Port Detection)
```bash
runa status
```

### View logs
```bash
# View existing logs
runa logs my-api

# Follow logs in real-time
runa logs my-api --follow
```

### Clear logs manually
```bash
runa flush my-api
```

### Stop a process
```bash
runa stop my-api
```

### Stop all processes
```bash
runa stop --all
```

### Restart a process
```bash
runa restart my-api
```

## Configuration

Runa stores its state and logs in `~/.runa/`.

### Restart Policies
- `always` (default): Always restart the process when it exits.
- `on-failure`: Restart only if the process exits with a non-zero exit code.
- `never`: Do not restart the process.

## License

MIT
