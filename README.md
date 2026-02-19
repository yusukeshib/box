# box

[日本語](README.ja.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

Safe, disposable dev environments for AI coding agents — powered by Docker and git. Supports `--local` mode for Docker-free git workspace sessions.

![demo](./demo.gif)

## Why box?

AI coding agents (Claude Code, Cursor, Copilot) are powerful — but letting them loose on your actual working tree is risky. Box gives them a **safe, isolated sandbox** where they can go wild without consequences.

- **Your code stays safe** — an independent clone is used, host files are never modified
- **AI agents can experiment freely** — commit, branch, rewrite, break things — your working tree is untouched
- **Persistent sessions** — exit and resume where you left off, files are preserved
- **Named sessions** — run multiple experiments in parallel
- **Bring your own toolchain** — works with any Docker image
- **Local mode** — use `--local` for Docker-free git workspace sessions

## Requirements

- [Git](https://git-scm.com/)
- [Docker](https://www.docker.com/) (or [OrbStack](https://orbstack.dev/) on macOS) — optional when using `--local` mode

## Install

### Quick install

```bash
curl -fsSL https://raw.githubusercontent.com/yusukeshib/box/main/install.sh | bash
```

### From crates.io

```bash
cargo install box-cli
```

### From source

```bash
cargo install --git https://github.com/yusukeshib/box
```

### Nix

```bash
nix run github:yusukeshib/box
```

### Binary download

Pre-built binaries are available on the [GitHub Releases](https://github.com/yusukeshib/box/releases) page.

## Quick Start

```bash
box my-feature
# Shortcut for `box create my-feature` — creates a new isolated session

box my-feature --local
# Creates a local session (git workspace only, no Docker)
```

Box must be run inside a git repository — it clones the current repo into the workspace.

For a zero-flags workflow, see [Custom Image Setup](#custom-image-setup) below.

## Custom Image Setup

The recommended way to use box: build your image once, set a couple of env vars, and never pass flags again.

**1. Create a Dockerfile with your toolchain**

Include whatever tools your workflow needs (languages, runtimes, CLI tools, etc.).

**2. Build the image**

```bash
docker build -t mydev .
```

**3. Set environment variables**

Add these to your `.zshrc` or `.bashrc`:

```bash
export BOX_DEFAULT_IMAGE=mydev              # your custom image
export BOX_DOCKER_ARGS="--network host"     # any extra Docker flags you always want
export BOX_DEFAULT_CMD="bash"               # default command for new sessions
```

**4. Done — just use box**

With those env vars set, every session uses your custom image with zero flags:

```bash
# That's it. From now on:
box create feature-1
box create bugfix-auth
box create experiment-v2
# Each gets an isolated sandbox with your full toolchain.
```

## Usage

```bash
box                                               Session manager (TUI)
box <name> [--local]                              Shortcut for `box create <name>`
box create <name> [--local] [options] [-- cmd...] Create a new session
box resume <name> [-d] [--docker-args <args>]     Resume an existing session
box stop <name>                                   Stop a running session
box exec <name> -- <cmd...>                       Run a command in a running session
box list [options]                                List sessions (alias: ls)
box remove <name>                                 Remove a session
box cd <name>                                     Print host project directory
box path <name>                                   Print workspace path
box origin                                        Cd back to origin project from workspace
box config zsh|bash                               Output shell completions
box upgrade                                       Upgrade to latest version
```

### Session manager

Running `box` with no arguments opens an interactive TUI:

```
 NAME            PROJECT      MODE    STATUS   CMD      IMAGE            CREATED
  New box...
> my-feature     /U/y/p/app   docker  running  claude   alpine:latest    2026-02-07 12:00:00 UTC
  local-exp      /U/y/p/app   local                                      2026-02-07 12:15:00 UTC
  test           /U/y/p/other docker                    ubuntu:latest    2026-02-07 12:30:00 UTC

 [Enter] Resume  [c] Cd  [o] Origin  [d] Delete  [q] Quit
```

- **Enter** on a session to resume it, or on "New box..." to create a new one
- **c** to cd to the session's workspace directory
- **o** to cd to the session's origin project directory
- **d** to delete the highlighted session (with confirmation)
- **q** / **Esc** to quit

### Create a session

```bash
# Shortcut: just pass a name
box my-feature

# Equivalent explicit form
box create my-feature

# Custom image with bash
box create my-feature --image ubuntu:latest -- bash

# Extra Docker flags (env vars, volumes, network, etc.)
box create my-feature --docker-args "-e KEY=VALUE -v /host:/container --network host"

# Create in detached mode (background)
box create my-feature -d -- claude -p "do something"

# Create a local session (no Docker required)
box create my-feature --local
```

### Resume a session

```bash
# Resume an existing session
box resume my-feature

# Resume in detached mode
box resume my-feature -d

# Detach without stopping: Ctrl+P, Ctrl+Q
```

### Run a command in a session

```bash
# Run a command in a running session
box exec my-feature -- ls -la

# Open a shell in a running session
box exec my-feature -- bash
```

### List sessions

```bash
# List all sessions
box list

# Alias
box ls

# Show only running sessions
box list --running

# Show only stopped sessions
box list --stopped

# Quiet mode — names only (useful for scripting)
box list -q --running

# Example: stop all running sessions
box stop $(box list -q --running)
```

### Cd to project directory

```bash
# Print the host project directory for a session
box cd my-feature

# With shell completions enabled, cd to the project directory
cd "$(box cd my-feature)"
```

### Navigate to origin project

```bash
# From inside a workspace, cd back to the origin project directory
box origin
```

### Stop and remove

```bash
# Stop a running session
box stop my-feature

# Remove a stopped session (container, workspace, and session data)
box remove my-feature
```

## Options

### `box create`

| Option | Description |
|--------|-------------|
| `-d` | Run container in the background (detached) |
| `--local` | Create a local session (git workspace only, no Docker) |
| `--image <image>` | Docker image to use (default: `alpine:latest`) |
| `--docker-args <args>` | Extra Docker flags (e.g. `-e KEY=VALUE`, `-v /host:/container`). Overrides `$BOX_DOCKER_ARGS` |
| `-- cmd...` | Command to run in container (default: `$BOX_DEFAULT_CMD` if set) |

### `box list`

| Option | Description |
|--------|-------------|
| `--running`, `-r` | Show only running sessions |
| `--stopped`, `-s` | Show only stopped sessions |
| `--quiet`, `-q` | Only print session names (useful for scripting) |

### `box resume`

| Option | Description |
|--------|-------------|
| `-d` | Resume in the background (detached) |
| `--docker-args <args>` | Extra Docker flags. Overrides `$BOX_DOCKER_ARGS` |

## Environment Variables

These let you configure defaults so you can skip CLI flags entirely. Set them in your `.zshrc` or `.bashrc` and every `box <name>` invocation uses them automatically.

| Variable | Description |
|----------|-------------|
| `BOX_DEFAULT_IMAGE` | Default Docker image for new sessions (default: `alpine:latest`) |
| `BOX_DOCKER_ARGS` | Default extra Docker flags, used when `--docker-args` is not provided |
| `BOX_DEFAULT_CMD` | Default command for new sessions, used when no `-- cmd` is provided |
| `BOX_MODE` | Set to `local` to create all sessions in local mode by default |

```bash
# Set default Docker flags for all sessions
export BOX_DOCKER_ARGS="--network host -v /data:/data:ro"
box create my-session

# Override with --docker-args for a specific session
box create my-session --docker-args "-e DEBUG=1"
```

## Shell Completions

Add one of these to your shell config to enable tab completion for session names and subcommands:

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

After reloading your shell, `box [tab]` will show available sessions and subcommands.

## How It Works

On first run, `git clone --local` creates an independent copy of your repo in the workspace directory. The container gets a fully self-contained git repo — no special mounts or entrypoint scripts needed. Your host working directory is never modified.

- **Independent clone** — Each session gets its own complete git repo via `git clone --local`
- **Persistent workspace** — Files survive `exit` and `box resume <name>` picks up where you left off; cleaned up with `box remove`
- **Any image, any user** — Works with root and non-root container images

| Aspect | Protection |
|--------|------------|
| Host working tree | Never modified — workspace is an independent clone |
| Workspace | Bind-mounted from `~/.box/workspaces/<name>/`, persists across stop/start |
| Session cleanup | `box remove` deletes container, workspace, and session data |

## Design Decisions

<details>
<summary><strong>Why <code>git clone --local</code>?</strong></summary>

Several git isolation strategies exist — here's why the alternatives fall short:

| Strategy | Problem |
|----------|---------|
| **Bind-mount the host repo** | No isolation at all; the agent modifies your actual files |
| **git worktree** | Shares the `.git` directory with the host; checkout, reset, and rebase can affect host branches and refs |
| **Bare-git mount** | Still shares state; branch creates/deletes in the container affect the host |
| **Branch-only isolation** | Nothing stops the agent from checking out other branches or running destructive git commands on shared refs |
| **Full copy (`cp -r`)** | Truly isolated but slow for large repos |

`git clone --local` wins because it's:

- **Fully independent** — the clone has its own `.git`; nothing in the container can touch the host repo
- **Fast** — hardlinks file objects on the same filesystem instead of copying
- **Complete** — full history, all branches, standard git repo
- **Simple** — no wrapper scripts or special entrypoints needed

</details>

<details>
<summary><strong>Why plain Docker?</strong></summary>

Some tools (e.g. Claude Code's `--sandbox`) provide built-in Docker sandboxing. Box takes a different approach — using plain Docker directly — which unlocks:

- **Your own toolchain** — use any Docker image with the exact languages, runtimes, and tools you need
- **Persistent sessions** — exit and resume where you left off; files and state are preserved
- **Full Docker control** — custom network, volumes, env vars, and any other `docker run` flags
- **Works with any agent** — not tied to a specific tool; use Claude Code, Cursor, Copilot, or manual workflows

Plain Docker gives full control while box handles the isolation and lifecycle.

</details>

## Security Note

The `--docker-args` flag and `BOX_DOCKER_ARGS` environment variable pass arguments directly to `docker run`. This means flags like `--privileged`, `--pid=host`, or `-v /:/host` can weaken or bypass container sandboxing. Only use trusted values and be careful when sourcing `BOX_DOCKER_ARGS` from shared or automated environments.

## Claude Code Integration

Box is the ideal companion for [Claude Code](https://docs.anthropic.com/en/docs/claude-code). Run Claude Code inside a box session and let it make risky changes, experiment with branches, and run tests — all fully isolated from your host.

```bash
box create ai-experiment --image node:20 -- claude
```

Run in the background with detach mode:

```bash
box create ai-experiment -d --image node:20 -- claude -p "refactor the auth module"
```

Everything the agent does stays inside the container. When you're done, delete the session and it's gone.

## License

MIT
