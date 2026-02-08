# realm

Sandboxed Docker environments for git repos.

## How it works

Realm mounts your repo's `.git` directory into a Docker container and checks out a dedicated branch. Your host working directory is never modified.

- **`.git`-only mount** — The container gets full git functionality (commit, branch, diff) without touching your working tree
- **Session isolation** — Each session works independently inside the container
- **Host stays clean** — After container exit, realm runs `git reset` to fix the host index

## Install

### From source (requires Rust toolchain)

```bash
cargo install --git https://github.com/yusukeshib/realm
```

### From crates.io

```bash
cargo install realm-cli
```

## Usage

```bash
realm switch                                        List all sessions
realm switch <name> [-- cmd...]                     Resume a session
realm switch -c <name> [options] [-- cmd...]        Create a new session
realm remove <name>                                 Delete session
```

### Create a session

```bash
# Default: alpine/git image, sh shell, current directory
realm switch -c my-feature

# Specify a project directory
realm switch -c my-feature -d ~/projects/my-app

# Custom image with bash
realm switch -c my-feature --image ubuntu:latest -- bash

# Build from a Dockerfile
realm switch -c my-feature --dockerfile ./Dockerfile -- bash

# Custom mount path inside container
realm switch -c my-feature --mount /src

# -c flag works in any position
realm switch my-feature -c --image ubuntu:latest -- bash
```

### Resume a session

```bash
realm switch my-feature
```

The container resumes with the same configuration from the original session.

### List sessions

```bash
realm switch
```

```
NAME                 PROJECT                        IMAGE                CREATED
----                 -------                        -----                -------
my-feature           /Users/you/projects/app        alpine/git           2026-02-07 12:00:00 UTC
test                 /Users/you/projects/other      ubuntu:latest        2026-02-07 12:30:00 UTC
```

### Remove a session

```bash
realm remove my-feature
```

This deletes the session metadata.

## Options

| Option | Description |
|--------|-------------|
| `-c` | Create a new session |
| `-d, --dir <path>` | Project directory (default: current directory) |
| `--image <image>` | Docker image to use (default: `alpine/git`) |
| `--dockerfile <path>` | Build image from a Dockerfile (mutually exclusive with `--image`) |
| `--mount <path>` | Mount path inside the container (default: `/workspace`) |

## Session Storage

Sessions are stored in `~/.realm/sessions/{name}/` with:
- `project_dir` — Absolute path to the git repo
- `image` — Docker image used
- `mount_path` — Container mount path
- `created_at` — Timestamp of session creation
- `resumed_at` — Timestamp of last resume (if applicable)
- `dockerfile` — Path to Dockerfile (if `--dockerfile` was used)
- `command` — Saved command args (if provided)

## Environment Variables

| Variable | Description |
|----------|-------------|
| `REALM_DOCKERFILE` | Default Dockerfile path (same as `--dockerfile`) |
| `REALM_DOCKER_ARGS` | Extra Docker flags (e.g., `--network host`, additional `-v` mounts) |

Examples:

```bash
# Always use your custom Dockerfile
export REALM_DOCKERFILE=~/my-realm/Dockerfile

# Pass extra Docker flags
REALM_DOCKER_ARGS="--network host -v /data:/data:ro" realm switch -c my-session
```

## Security Model

| Aspect | Protection |
|--------|------------|
| Host working tree | Never modified — only `.git` is mounted |
| Git data | Container works on mounted `.git` only |
| Container | Destroyed after each exit (`--rm`) |
| Host index | Restored via `git reset` after container exit |

## License

MIT
