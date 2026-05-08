# box

[日本語](README.ja.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

A CLI tool that manages sandboxed git workspaces. Multi-repo support included.

![demo](./demo.gif)

## What is box?

Box creates and manages named workspaces using `git worktree` (default) or `git clone --local`.

- Register repos with `box repo add` (bare-cloned to `~/.box/repos/`), then create sessions — each session sets up repos in `~/.box/workspaces/<session>/`
- Multiple repos can be grouped into a single session, or saved as a named **preset** for reuse
- Worktree mode is lightweight and fast; clone mode gives full `.git` isolation

## Features

- **Two workspace strategies** — `git worktree` (default, lightweight) or `git clone --local` (full isolation)
- **Multi-repo sessions** — group multiple repos into one workspace, optionally via reusable presets
- **Interactive TUI** — select repos, enter session name, with history
- **Shell integration** — completions, `cd` wrapper, and terminal-tab renaming for zsh/bash

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
box new my-feature --repo my-app

# 4. Switch into the workspace later
box switch my-feature

# 5. Clean up
box remove my-feature
```

## Usage

```bash
box                                        Interactive TUI (create new sessions)
box new <name> --repo <r> [options]        Create a new session
box edit <name> [--add <r>] [--remove <r>] Add/remove repos in a session
box list [options]                         List sessions (alias: ls)
box remove [<name>] [--all]                Remove a session (alias: rm)
box switch <name>                          Switch into the session workspace (aliases: cd, sw)
box rebase <branch>                        Fetch origin and rebase HEAD onto <branch>
box repo add [path]                        Register a git repo (bare clone)
box repo remove <name>                     Unregister a repo (alias: rm)
box repo list                              List registered repos (alias: ls)
box preset add <name> --repo <r>...        Create or update a preset
box preset edit <name>                     Edit repos in a preset (TUI)
box preset remove <name>                   Remove a preset (alias: rm)
box preset list                            List presets (alias: ls)
box config zsh|bash                        Output shell configuration
box upgrade                                Upgrade to latest version
```

`-v`/`--verbose` is a global flag (also via `BOX_VERBOSE=1`) that turns on detailed output.

### Create a session

```bash
# Default (worktree strategy)
box new my-feature --repo my-app

# Use clone strategy for full isolation
box new my-feature --repo my-app --strategy clone

# Multiple repos
box new my-feature --repo frontend --repo backend

# From a preset
box new my-feature --preset work
```

`--repo` (repeatable) or `--preset` is required. To create sessions interactively, run `box` with no arguments.

### Edit session repos

```bash
box edit my-feature                                # TUI: toggle repos
box edit my-feature --add app-c                    # non-interactive add
box edit my-feature --add app-c --remove app-a     # add and remove
```

`--add` and `--remove` may be repeated. When either is set, the TUI is skipped.

### List and manage sessions

```bash
box list                        # List all sessions
box ls                          # Alias
box list -q                     # Names only (for scripting)
box list -p                     # Only sessions for the current project
box remove my-feature           # Remove a session by name
box remove                      # Interactive selector (multi-select)
box remove --all                # Remove every session
```

### Switch into a workspace

```bash
box switch my-feature           # Switch into the session workspace
box cd my-feature               # Alias
box sw my-feature               # Alias
```

With shell integration enabled (`eval "$(box config zsh)"`), `box switch` changes your working directory and renames the current zellij/tmux tab to the session name. Without it, the workspace path is printed to stdout.

### Rebase the current branch

```bash
box rebase main                 # fetch origin in the bare repo, then rebase HEAD onto main
```

`box rebase` runs from inside any session worktree. It fetches the bare repo behind the worktree (handling sibling-worktree branches safely) and then runs `git rebase <branch>` in the current worktree.

## Multi-repo Workspaces

Register repos (bare-cloned to `~/.box/repos/`), then reference them by name when creating sessions:

```bash
box repo add ~/projects/frontend
box repo add ~/projects/backend

box new my-feature --repo frontend --repo backend
```

Repos are always fetched before session creation. Each repo is set up in `~/.box/workspaces/<session>/<repo>/`. For single-repo sessions, the workspace path resolves directly to the repo subdirectory.

### Presets

A preset is a named list of repos you create sessions from regularly:

```bash
box preset add work --repo frontend --repo backend   # define
box preset add work                                  # interactive selector
box preset edit work                                 # update repos (TUI)
box preset list                                      # list presets
box preset remove work                               # delete

box new my-feature --preset work                     # use it
```

## Options

### `box new`

| Option | Description |
|--------|-------------|
| `<name>` | Session name (required) |
| `--repo <name>` | Repos to include (repeatable; mutually exclusive with `--preset`) |
| `--preset <name>` | Use a preset (mutually exclusive with `--repo`) |
| `--strategy <strategy>` | `worktree` (default) or `clone` |

### `box edit`

| Option | Description |
|--------|-------------|
| `<name>` | Session name (required) |
| `--add <repo>` | Add a repo (repeatable; skips the TUI when set) |
| `--remove <repo>` | Remove a repo (repeatable; skips the TUI when set) |

### `box list`

| Option | Description |
|--------|-------------|
| `--project`, `-p` | Show only sessions for the current project directory |
| `--quiet`, `-q` | Only print session names |

### `box remove`

| Option | Description |
|--------|-------------|
| `<name>` | Session name (omit to open interactive selector) |
| `--all`, `-a` | Remove every session (conflicts with `<name>`) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BOX_STRATEGY` | Default workspace strategy (`worktree` or `clone`). Overridden by `--strategy` |
| `BOX_VERBOSE` | When set, equivalent to `--verbose` |
| `BOX_ROOT` | Override the box data directory (default `~/.box`). Read by the shell completions |

## Shell Integration

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

This provides:

- Tab completion for sessions, repos, and presets
- A `box` shell function so `box switch` / `box new` change your working directory
- Automatic zellij/tmux tab renaming to the session name when switching into or creating a session

## How It Works

Box supports two workspace strategies:

### Worktree (default)

```
~/.box/repos/             box new my-feature        ~/.box/workspaces/my-feature/
  frontend.git   ──── git worktree add ─────>      frontend/
  backend.git                                      backend/
```

`git worktree` creates a lightweight working tree linked to the bare repo. It shares the object store, so creation is instant and uses minimal disk space. Each worktree gets its own `box/<session>` branch, and `box remove` cleans up the worktree properly.

### Clone

```
~/.box/repos/             box new my-feature        ~/.box/workspaces/my-feature/
  frontend.git   ──── git clone --local ────>      frontend/
  backend.git                                      backend/
```

`git clone --local` creates a fully independent git repo using hardlinks for file objects. Each clone has its own `.git` directory — commits, branches, resets, and destructive operations in the workspace do not affect the original.

### Comparison

| | Worktree | Clone |
|---|---|---|
| Speed | Instant | Fast (hardlinks) |
| Disk usage | Minimal (shared objects) | Low (hardlinked objects) |
| Isolation | Separate working tree, shared `.git` | Fully independent `.git` |
| Best for | Feature branches, quick experiments | Full isolation, destructive operations |

| Aspect | Detail |
|--------|--------|
| Bare repos | `~/.box/repos/<name>.git/` |
| Workspace location | `~/.box/workspaces/<session>/` |
| Session metadata | `~/.box/sessions/<session>/` |
| Presets | `~/.box/presets/<name>` |
| Default strategy | `git worktree` (override with `--strategy clone`) |
| Cleanup | `box remove` deletes workspace and session data |

## License

MIT
