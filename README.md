# realm

Sandboxed Docker environments for git repos.

## How it works

Realm mounts your repo's `.git` directory into a Docker container and checks out a dedicated branch. Your host working directory is never modified.

- **`.git`-only mount** — The container gets full git functionality (commit, branch, diff) without touching your working tree
- **Branch isolation** — Each session works on a `realm/{name}` branch. Commits persist in the host's `.git`
- **Host stays clean** — After container exit, realm runs `git reset` to fix the host index

## Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/yusukeshib/realm/main/install.sh | bash
```

Ensure `~/.local/bin` is in your PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Usage

```bash
realm new <name> [options] [project_dir] [-- cmd...]   Create a new session
realm resume <name> [-- cmd...]                         Resume existing session
realm list                                               List all sessions
realm remove <name>                                      Delete session
realm upgrade                                            Upgrade to latest version
```

### Create a session

```bash
# Default: alpine/git image, sh shell, current directory
realm new my-feature

# Specify a project directory
realm new my-feature ~/projects/my-app

# Custom image with bash
realm new my-feature --image ubuntu:latest -- bash

# Build from a Dockerfile
realm new my-feature --dockerfile ./Dockerfile -- bash

# Custom mount path inside container
realm new my-feature --mount /src
```

### Resume a session

```bash
realm resume my-feature
```

The container checks out the `realm/my-feature` branch, which has all previous commits from earlier sessions.

### List sessions

```bash
realm list
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

This deletes the session metadata and the `realm/my-feature` branch.

## Options

| Option | Description |
|--------|-------------|
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
REALM_DOCKER_ARGS="--network host -v /data:/data:ro" realm new my-session
```

## Security Model

| Aspect | Protection |
|--------|------------|
| Host working tree | Never modified — only `.git` is mounted |
| Git data | Isolated on `realm/{name}` branch |
| Container | Destroyed after each exit (`--rm`) |
| Host index | Restored via `git reset` after container exit |

## License

MIT
