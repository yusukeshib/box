# box

[日本語](README.ja.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

Isolated git workspaces with a built-in terminal multiplexer. Clone, branch, break things — your repo stays untouched.

![demo](./demo.gif)

## Why box?

Box gives you **isolated** git workspaces with **persistent** terminal sessions — two core ideas:

**1. `git clone --local` for instant isolation**

Each session gets its own independent git repo — a full clone with all history and branches, created via hardlinks so it's fast even for large repos. Nothing you do in a box session can affect your original repository.

**2. Built-in terminal multiplexer for persistent sessions**

Every session runs inside a terminal multiplexer with scrollback, mouse support, and a persistent connection. Detach and reattach freely — your process keeps running in the background.

```
 COMMAND  ^P/^N scroll  ^Q detach  ^X stop  Esc exit              x
$ make test
...
```

## Features

- **Isolated git workspaces** — `git clone --local` creates an independent repo per session; host files are never modified
- **Persistent sessions** — detach with `Ctrl+P` → `Ctrl+Q`, reattach with `box resume`; processes keep running
- **Terminal multiplexer** — scrollback history, mouse scroll, scrollbar, COMMAND mode for navigation
- **Named sessions** — run multiple experiments in parallel, each with its own workspace
- **Session manager TUI** — interactive dashboard to create, resume, and manage sessions
- **Docker mode** — optional full container isolation with any Docker image (`BOX_MODE=docker`)

## Requirements

- [Git](https://git-scm.com/)
- [Docker](https://www.docker.com/) (or [OrbStack](https://orbstack.dev/) on macOS) — only required for `BOX_MODE=docker`

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
# Creates an isolated git workspace and opens a shell inside it
```

Box must be run inside a git repository. It clones the current repo into `~/.box/workspaces/<name>/`.

```bash
# Work in the isolated workspace...
$ git checkout -b experiment
$ make test  # break things freely

# Detach (Ctrl+P enters COMMAND mode, then Ctrl+Q)
# Your process keeps running in the background

# Reattach later
box resume my-feature

# Done? Clean up
box remove my-feature
```

## Terminal Multiplexer

Every box session runs inside a built-in terminal multiplexer. This gives you session persistence, scrollback, and keyboard-driven navigation.

### COMMAND mode

Press `Ctrl+P` (configurable) to enter COMMAND mode. The header bar turns dark gray and shows available keys:

```
 COMMAND  ^P/^N scroll  ^Q detach  ^X stop  Esc exit              x
```

| Key | Action |
|-----|--------|
| `Ctrl+P` | Scroll up 1 line |
| `Ctrl+N` | Scroll down 1 line |
| `Ctrl+U` | Scroll up half page |
| `Ctrl+D` | Scroll down half page |
| `Arrow keys` | Scroll up/down |
| `PgUp` / `PgDn` | Scroll by half page |
| `Ctrl+Q` | Detach from session (process keeps running) |
| `Ctrl+X` | Stop/kill the session |
| `Esc` | Exit COMMAND mode (snap to bottom) |

Mouse scroll works in both normal and COMMAND mode. A scrollbar appears when there is scrollback content.

### Configuring the prefix key

The key that enters COMMAND mode can be changed via `~/.config/box/config.toml`:

```toml
[mux]
prefix_key = "Ctrl+B"   # default: "Ctrl+P"
```

Supports `Ctrl+A` through `Ctrl+Z`.

## Session Manager

Running `box` with no arguments opens an interactive TUI:

```
 NAME            PROJECT      MODE    STATUS   CMD      IMAGE            CREATED
  New box...
> my-feature     /U/y/p/app   local   running  bash                      2026-02-07 12:00:00 UTC
  experiment     /U/y/p/app   local                                      2026-02-07 12:15:00 UTC
  docker-test    /U/y/p/other docker                    ubuntu:latest    2026-02-07 12:30:00 UTC

 [Enter] Resume  [c] Cd  [o] Origin  [d] Delete  [q] Quit
```

- **Enter** — resume a session, or create a new one on "New box..."
- **c** — cd to the session's workspace directory
- **o** — cd to the session's origin project directory
- **d** — delete the highlighted session (with confirmation)
- **q** / **Esc** — quit

## Usage

```bash
box                                               Session manager (TUI)
box <name> [--local] [--docker]                   Shortcut for `box create <name>`
box create <name> [--local] [--docker] [options] [-- cmd...] Create a new session
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

### Create a session

```bash
# Shortcut: just pass a name
box my-feature

# With a specific command
box create my-feature -- make test

# Create in detached mode (background)
box create my-feature -d -- long-running-task
```

### Resume a session

```bash
box resume my-feature

# Resume in detached mode
box resume my-feature -d
```

### List and manage sessions

```bash
box list                        # List all sessions
box ls                          # Alias
box list --running              # Only running sessions
box list -q --running           # Names only (for scripting)
box stop my-feature             # Stop a session
box remove my-feature           # Remove session, workspace, and data
box stop $(box list -q --running)  # Stop all running sessions
```

### Navigate between workspaces

```bash
box cd my-feature               # Print the host project directory
cd "$(box path my-feature)"    # cd to the workspace
box origin                      # From workspace, cd back to origin
```

## Docker Mode

For full container isolation, set `BOX_MODE=docker`. Each session runs inside a Docker container with the workspace bind-mounted.

```bash
export BOX_MODE=docker
```

Optionally configure a custom image and defaults:

```bash
export BOX_DEFAULT_IMAGE=mydev              # your custom image
export BOX_DOCKER_ARGS="--network host"     # extra Docker flags
export BOX_DEFAULT_CMD="bash"               # default command
```

```bash
# Docker sessions with explicit options
box create my-feature --docker --image ubuntu:latest -- bash
box create my-feature --docker --docker-args "-e KEY=VALUE -v /host:/container"
```

## Options

### `box create`

| Option | Description |
|--------|-------------|
| `-d` | Run in the background (detached) |
| `--local` | Create a local session (default) |
| `--docker` | Create a Docker session (requires Docker) |
| `--image <image>` | Docker image to use (default: `alpine:latest`) |
| `--docker-args <args>` | Extra Docker flags (e.g. `-e KEY=VALUE`, `-v /host:/container`). Overrides `$BOX_DOCKER_ARGS` |
| `-- cmd...` | Command to run (default: `$BOX_DEFAULT_CMD` if set) |

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

| Variable | Description |
|----------|-------------|
| `BOX_DEFAULT_IMAGE` | Default Docker image for new sessions (default: `alpine:latest`) |
| `BOX_DOCKER_ARGS` | Default extra Docker flags, used when `--docker-args` is not provided |
| `BOX_DEFAULT_CMD` | Default command for new sessions, used when no `-- cmd` is provided |
| `BOX_MODE` | Session mode: `local` (default) or `docker` |

## Shell Completions

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

## How It Works

```
your-repo/          box create my-feature         ~/.box/workspaces/my-feature/
  .git/        ──── git clone --local ────>         .git/  (independent)
  src/                                              src/   (hardlinked)
  ...                                               ...
```

`git clone --local` creates a fully independent git repo using hardlinks for file objects. The clone has its own `.git` directory — commits, branches, resets, and destructive operations in the workspace cannot affect your original repository.

The built-in terminal multiplexer wraps each session with:
- **Session persistence** — the process runs in a background server; detach and reattach without interruption
- **Scrollback** — 10,000 lines of history with keyboard and mouse navigation
- **Header bar** — shows session name, scroll position, and COMMAND mode status

| Aspect | Detail |
|--------|--------|
| Workspace location | `~/.box/workspaces/<name>/` |
| Session metadata | `~/.box/sessions/<name>/` |
| Git isolation | Full — independent `.git`, no shared refs |
| Session persistence | Multiplexer server keeps process alive across detach/reattach |
| Cleanup | `box remove` deletes workspace, session data, and container (if Docker) |

## Design Decisions

<details>
<summary><strong>Why <code>git clone --local</code>?</strong></summary>

| Strategy | Problem |
|----------|---------|
| **Bind-mount the host repo** | No isolation at all; the agent modifies your actual files |
| **git worktree** | Shares the `.git` directory with the host; checkout, reset, and rebase can affect host branches and refs |
| **Bare-git mount** | Still shares state; branch creates/deletes in the container affect the host |
| **Branch-only isolation** | Nothing stops destructive git commands on shared refs |
| **Full copy (`cp -r`)** | Truly isolated but slow for large repos |

`git clone --local` is fully independent (own `.git`), fast (hardlinks), complete (full history), and simple (no wrapper scripts).

</details>

<details>
<summary><strong>Why a built-in multiplexer?</strong></summary>

Box needs session persistence — the ability to detach from a running process and reattach later. Rather than requiring tmux or screen as external dependencies, box includes a purpose-built terminal multiplexer that:

- Requires zero configuration — works out of the box
- Provides a consistent UI across all sessions (header bar, scrollback, COMMAND mode)
- Handles the client-server architecture for session persistence transparently
- Supports mouse scroll and a visual scrollbar for navigating output history

</details>

<details>
<summary><strong>Why plain Docker?</strong></summary>

Some tools provide built-in Docker sandboxing. Box uses plain Docker directly, which gives you:

- **Your own toolchain** — any Docker image with the exact tools you need
- **Full Docker control** — custom network, volumes, env vars, and any `docker run` flags
- **Works with any workflow** — not tied to a specific tool or agent

</details>

## Claude Code Integration

Box works well with [Claude Code](https://docs.anthropic.com/en/docs/claude-code) for running AI agents in isolated workspaces:

```bash
box create ai-experiment -- claude
box create ai-experiment -d -- claude -p "refactor the auth module"
```

Everything the agent does stays in the workspace. Delete the session when you're done.

## Security Note

The `--docker-args` flag and `BOX_DOCKER_ARGS` environment variable pass arguments directly to `docker run`. Flags like `--privileged` or `-v /:/host` can weaken container sandboxing. Only use trusted values.

## License

MIT
