# box

[日本語](README.ja.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

A CLI tool that manages workspaces via `git clone --local`. Multi-repo support included.

![demo](./demo.gif)

## What is box?

Box creates and manages named workspaces using `git clone --local`.

- Register repos with `box repo add`, then create sessions — each session clones repos into `~/.box/workspaces/<session>/`
- Multiple repos can be grouped into a single session
- Each clone has its own `.git`, so the original repo is never affected

## Features

- **`git clone --local` workspaces** — fast cloning with hardlinks, fully independent `.git`
- **Multi-repo sessions** — group multiple repos into one workspace
- **Interactive TUI** — select repos, enter session name and command, with history
- **Shell integration** — completions and `cd` wrapper for zsh/bash

## Requirements

- [Git](https://git-scm.com/)

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
# 1. Register a repo
box repo add ~/projects/my-app

# 2. Create a session via TUI
box

# 3. Or create via CLI
box new my-feature --repo my-app -- make test

# 4. Clean up
box remove my-feature
```

## Usage

```bash
box                                    Interactive TUI (create new sessions)
box new <name> --repo <r> [-- cmd...]  Create a new session
box edit <name>                        Add/remove repos in a session
box exec <name> -- <cmd...>            Run a command in a session workspace
box list [options]                     List sessions (alias: ls)
box remove <name>                      Remove a session and its workspace
box cd <name>                          cd into the session workspace
box repo add [path]                    Register a git repo
box repo remove <name>                 Unregister a repo
box repo list                          List registered repos (alias: ls)
box config zsh|bash                    Output shell configuration
box upgrade                            Upgrade to latest version
```

### Create a session

```bash
# With a command
box new my-feature --repo my-app -- make test

# Multiple repos
box new my-feature --repo frontend --repo backend

# Minimal
box new my-feature --repo my-app
```

`--repo` is required. To create sessions interactively, use `box` (no arguments) to launch the TUI.

### Edit session repos

```bash
box edit my-feature             # Add/remove repos in an existing session
```

### List and manage sessions

```bash
box list                        # List all sessions
box ls                          # Alias
box list -q                     # Names only (for scripting)
box list -p                     # Only sessions for the current project
box remove my-feature           # Remove session and workspace
```

### Navigate to a workspace

```bash
box cd my-feature               # cd into the session workspace
```

With shell integration enabled (`eval "$(box config zsh)"`), `box cd` changes your working directory. Without it, the workspace path is printed to stdout.

## Multi-repo Workspaces

Register repos, then reference them by name when creating sessions:

```bash
box repo add ~/projects/frontend
box repo add ~/projects/backend

box new my-feature --repo frontend --repo backend
```

Each repo is cloned into `~/.box/workspaces/<session>/<repo>/`. For single-repo sessions, the workspace path resolves directly to the repo subdirectory.

## Options

### `box new`

| Option | Description |
|--------|-------------|
| `<name>` | Session name (required) |
| `--repo <name>` | Repos to clone (required, repeatable) |
| `-- cmd...` | Command to run in the workspace (default: `$BOX_DEFAULT_CMD` if set) |

### `box list`

| Option | Description |
|--------|-------------|
| `--project`, `-p` | Show only sessions for the current project directory |
| `--quiet`, `-q` | Only print session names |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BOX_DEFAULT_CMD` | Default command for new sessions, used when no `-- cmd` is provided |

## Shell Integration

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

This provides tab completions and a `box` shell function that enables `box cd` to change your working directory.

## How It Works

```
registered repos      box new my-feature        ~/.box/workspaces/my-feature/
  frontend/      ──── git clone --local ────>      frontend/
  backend/                                         backend/
```

`git clone --local` creates an independent git repo using hardlinks for file objects. Each clone has its own `.git` directory — commits, branches, resets, and destructive operations in the workspace do not affect the original.

| Aspect | Detail |
|--------|--------|
| Workspace location | `~/.box/workspaces/<session>/` |
| Session metadata | `~/.box/sessions/<session>/` |
| Clone method | `git clone --local` (independent `.git`, hardlinked objects) |
| Cleanup | `box remove` deletes workspace and session data |

## Design Decisions

<details>
<summary><strong>Why <code>git clone --local</code>?</strong></summary>

| Alternative | Trade-off |
|-------------|-----------|
| **Bind-mount** | No isolation; modifications affect original files |
| **git worktree** | Shares `.git` with host; checkout/reset/rebase can affect host refs |
| **Bare-git mount** | Shares state; branch operations affect host |
| **Full copy (`cp -r`)** | Independent but slow for large repos |

`git clone --local` is independent (own `.git`), fast (hardlinks), complete (full history), and simple (no wrapper scripts).

</details>

## Claude Code Integration

Box works well with [Claude Code](https://docs.anthropic.com/en/docs/claude-code) for running AI agents in independent workspaces:

```bash
box new ai-experiment --repo my-app -- claude
```

Everything the agent does stays in the workspace. Delete the session when done.

## License

MIT
