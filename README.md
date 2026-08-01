# box


[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

A CLI tool that manages sandboxed git workspaces. Multi-repo support included.

![demo](./demo.gif)

## What is box?

Box creates and manages named workspaces using `git worktree` (default) or `git clone --local`.

- Register sources with `box source add` (bare-cloned to `~/.box/repos/`), then create workspaces — each workspace sets up repos in `~/.box/workspaces/<name>/`
- Multiple repos can be grouped into a single session, or saved as a named **preset** for reuse
- Worktree mode is lightweight and fast; clone mode gives full `.git` isolation

## Features

- **Two workspace strategies** — `git worktree` (default, lightweight) or `git clone --local` (full isolation)
- **Multi-repo sessions** — group multiple repos into one workspace, optionally via reusable presets
- **Age-based pruning** — clean up workspaces older than one day by default
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
# 1. Register a source
box source add ~/projects/my-app

# 2. Create a workspace
box workspace add my-feature --repo my-app

# 3. Switch into the workspace later
box workspace switch my-feature

# 4. Clean up
box workspace remove my-feature
```

## Usage

Box has a three-tier model: a **source** is a registered upstream git repo, a
**workspace** is a named sandbox built from one or more sources, and a **repo**
is a source checked out inside a workspace (or listed in a preset). Every
command targets an explicit object — there is no implicit current-directory
resolution.

```bash
box workspace add <name> --repo <r> [opts]  Create a workspace (alias: ws)
box workspace list [-q]                     List workspaces (alias: ls)
box workspace remove [<name>] [--older-than <age>]  Remove or prune workspaces (alias: rm)
box workspace switch <name>                 Switch into a workspace (alias: sw)
box repo add <r>... --workspace|--preset <name>     Add repo(s) to a workspace/preset
box repo remove <r>... --workspace|--preset <name>  Remove repo(s) (alias: rm)
box repo list --workspace|--preset <name>           List repos (alias: ls)
box source add <url|path>                   Register a source (bare clone; `.` = cwd)
box source remove <name>                    Unregister a source (alias: rm)
box source list                             List registered sources (alias: ls)
box preset add <name> --repo <r>...         Create a preset
box preset update <name> --repo <r>...      Replace a preset's repos
box preset remove <name>                    Remove a preset (alias: rm)
box preset list                             List presets (alias: ls)
box rebase <branch> --workspace <name> --repo <r>   Fetch origin and rebase a repo
box config zsh|bash                         Output shell configuration
box upgrade                                 Upgrade to latest version
```

Subcommand aliases: `workspace`=`ws`, and within each group `list`=`ls`,
`remove`=`rm`, `switch`=`sw`.

`-v`/`--verbose` is a global flag (also via `BOX_VERBOSE=1`) that turns on detailed output.

### Create a workspace

```bash
# Default (worktree strategy)
box workspace add my-feature --repo my-app

# Use clone strategy for full isolation
box workspace add my-feature --repo my-app --strategy clone

# Multiple repos
box workspace add my-feature --repo frontend --repo backend

# From a preset
box workspace add my-feature --preset work
```

`--repo` (repeatable) or `--preset` is required. Running `box` without a command displays help.

### Add / remove repos in a workspace

```bash
box repo add app-c --workspace my-feature            # add one repo
box repo add app-c app-d --workspace my-feature      # add several
box repo remove app-a --workspace my-feature         # remove a repo
box repo list --workspace my-feature                 # list repos in the workspace
```

The same `box repo` group manages preset membership — swap `--workspace <name>`
for `--preset <name>`. Exactly one of `--workspace` / `--preset` is required.

### List and manage workspaces

```bash
box workspace list              # List all workspaces
box workspace ls                # Alias
box workspace list -q           # Names only (for scripting)
box workspace remove my-feature          # Remove a workspace by name
box workspace remove                     # Prune workspaces at least 1 day old
box workspace remove --older-than 7d     # Prune workspaces at least 7 days old
box workspace remove --all               # Remove every workspace
```

### Switch into a workspace

```bash
box workspace switch my-feature # Switch into the workspace
box workspace sw my-feature     # Alias
box ws sw my-feature            # `ws` is an alias of `workspace`
```

With shell integration enabled (`eval "$(box config zsh)"`), `box workspace switch` changes your working directory. If you set `BOX_POST_SWITCH_HOOK`, it also runs that hook with the workspace name (see [Shell Integration](#shell-integration)). Without shell integration, the workspace path is printed to stdout.

### Rebase a workspace repo

```bash
box rebase main --workspace my-feature --repo my-app   # fetch origin, then rebase onto main
```

`box rebase` targets an explicit workspace repo. It fetches the bare repo behind the worktree (handling sibling-worktree branches safely) and then runs `git rebase <branch>` in that repo's worktree.

## Multi-repo Workspaces

Register sources (bare-cloned to `~/.box/repos/`), then reference them by name when creating workspaces:

```bash
box source add ~/projects/frontend
box source add ~/projects/backend

box workspace add my-feature --repo frontend --repo backend
```

Repos are always fetched before workspace creation. Each repo is set up in `~/.box/workspaces/<workspace>/<repo>/`. For single-repo workspaces, the workspace path resolves directly to the repo subdirectory.

### Presets

A preset is a named list of repos you create sessions from regularly:

```bash
box preset add work --repo frontend --repo backend   # define
box repo add api --preset work                       # add a repo to the preset
box repo remove backend --preset work                # remove a repo from the preset
box repo list --preset work                          # list the preset's repos
box preset list                                      # list presets
box preset remove work                               # delete

box workspace add my-feature --preset work           # use it
```

## Options

### `box workspace add`

| Option | Description |
|--------|-------------|
| `<name>` | Workspace name (required) |
| `--repo <name>` | Repos to include (repeatable; mutually exclusive with `--preset`) |
| `--preset <name>` | Use a preset (mutually exclusive with `--repo`) |
| `--strategy <strategy>` | `worktree` (default) or `clone` |

### `box repo add` / `box repo remove` / `box repo list`

| Option | Description |
|--------|-------------|
| `<repo>...` | Repo name(s) (required for add/remove) |
| `--workspace <name>` | Target workspace (mutually exclusive with `--preset`) |
| `--preset <name>` | Target preset (mutually exclusive with `--workspace`) |

Exactly one of `--workspace` / `--preset` must be given.

### `box workspace list`

| Option | Description |
|--------|-------------|
| `--quiet`, `-q` | Only print workspace names |

### `box workspace remove`

| Option | Description |
|--------|-------------|
| `<name>` | Workspace name; omit to prune old workspaces |
| `--older-than <age>` | Minimum age for pruning (`s`, `m`, `h`, or `d`; default: `1d`) |
| `--all`, `-a` | Remove every workspace (conflicts with `<name>`) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BOX_STRATEGY` | Default workspace strategy (`worktree` or `clone`). Overridden by `--strategy` |
| `BOX_VERBOSE` | When set, equivalent to `--verbose` |
| `BOX_ROOT` | Override the box data directory (default `~/.box`). Read by the shell completions |
| `BOX_POST_SWITCH_HOOK` | Shell snippet run after `box workspace switch` / `box workspace add`. The workspace name is available as `$BOX_SESSION_NAME`. See [Shell Integration](#shell-integration) |

## Shell Integration

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

This provides:

- Tab completion for workspaces, sources, and presets
- A `box` shell function so `box workspace switch` / `box workspace add` change your working directory
- A `BOX_POST_SWITCH_HOOK` that runs after switching into or creating a workspace, with the workspace name in `$BOX_SESSION_NAME`

### Post-switch hook

Set `BOX_POST_SWITCH_HOOK` to any shell snippet you want to run when a workspace is entered (fires on both `box workspace add` and `box workspace switch`). Common choices:

```bash
# tmux — rename the current window
export BOX_POST_SWITCH_HOOK='tmux rename-window "$BOX_SESSION_NAME"'

# zellij — rename the current tab
export BOX_POST_SWITCH_HOOK='zellij action rename-tab "$BOX_SESSION_NAME"'

# kitty — set the tab title
export BOX_POST_SWITCH_HOOK='kitty @ set-tab-title "$BOX_SESSION_NAME"'

# Generic OSC 2 — set the terminal window/tab title
export BOX_POST_SWITCH_HOOK='printf "\033]2;%s\007" "$BOX_SESSION_NAME"'
```

The snippet runs in the current shell via `eval`, so `$TMUX`, `$ZELLIJ`, and other environment variables are available for branching.

## How It Works

Box supports two workspace strategies:

### Worktree (default)

```
~/.box/repos/         box workspace add my-feature  ~/.box/workspaces/my-feature/
  frontend.git   ──── git worktree add ─────>      frontend/
  backend.git                                      backend/
```

`git worktree` creates a lightweight working tree linked to the bare repo. It shares the object store, so creation is instant and uses minimal disk space. Each worktree gets its own `box/<name>` branch, and `box workspace remove` cleans up the worktree properly.

### Clone

```
~/.box/repos/         box workspace add my-feature  ~/.box/workspaces/my-feature/
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
| Cleanup | `box workspace remove` deletes workspace and session data |

## License

MIT
