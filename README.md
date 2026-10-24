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
box workspace remove <name>...|--all           Remove workspace(s) (alias: rm)
box workspace prune [--older-than <age>]    Remove workspaces older than an age (default: 3d)
box workspace update <name>...|--all        Fetch origin for workspace repos
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

# Use layer strategy (macOS) for fast creation of large repos
box workspace add my-feature --repo my-app --strategy layer

# Multiple repos
box workspace add my-feature --repo frontend --repo backend

# From a preset
box workspace add my-feature --preset work

# Skip fetching sources first (faster; branches from the local refs)
box workspace add my-feature --repo my-app --no-update
```

`--repo` (repeatable) or `--preset` is required. Running `box` without a command displays help.

By default each source is fetched from `origin` before the workspace is created.
Set `BOX_UPDATE=no` to make `--no-update` the default; `--update` forces a fetch
for one command. Accepted values are `yes`/`no`, `true`/`false`, `on`/`off`, and
`1`/`0`. `--no-fetch` is still accepted as an alias of `--no-update`.

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
box workspace rm a b c d                 # Remove multiple workspaces
box workspace remove --all               # Remove every workspace
box workspace prune                      # Remove workspaces at least 3 days old
box workspace prune --older-than 7d      # Remove workspaces at least 7 days old
```

### Switch into a workspace

```bash
box workspace switch my-feature # Switch into the workspace
box workspace sw my-feature     # Alias
box ws sw my-feature            # `ws` is an alias of `workspace`
```

With shell integration enabled (`eval "$(box config zsh)"`), `box workspace switch` changes your working directory. Without shell integration, the workspace path is printed to stdout.

### Update workspace repos

```bash
box workspace update my-feature   # fetch origin for every repo in a workspace
box workspace update a b          # several workspaces
box workspace update --all        # every workspace
```

`box workspace update` refreshes `origin/*` refs and never touches working trees
or checked-out branches. Worktree and layer workspaces fetch the shared bare
repo once per source, excluding branches checked out in any workspace (a plain
`git fetch` there can refuse to run when upstream has a branch with the same
name as a checked-out one). Clone workspaces run `git fetch --prune origin` in
each clone. Then rebase or merge with plain git, for example
`git rebase origin/main`.

## Multi-repo Workspaces

Register sources (bare-cloned to `~/.box/repos/`), then reference them by name when creating workspaces:

```bash
box source add ~/projects/frontend
box source add ~/projects/backend

box workspace add my-feature --repo frontend --repo backend
```

Repos are fetched before workspace creation unless `--no-update` or `BOX_UPDATE=no` is set. Each repo is set up in `~/.box/workspaces/<workspace>/<repo>/`. For single-repo workspaces, the workspace path resolves directly to the repo subdirectory.

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
| `--strategy <strategy>` | `worktree` (default), `clone`, or `layer` (macOS) |

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
| `<name>...` | Workspace names (required unless `--all`) |
| `--all`, `-a` | Remove every workspace (conflicts with names) |

All names are checked before deletion; duplicate names are removed only once.
With shell integration, removing multiple workspaces returns to the original project
of the workspace containing the current directory, if it was removed. Otherwise,
the current directory is unchanged. Removing a single workspace retains the existing
behavior of returning to its original project.

### `box workspace prune`

| Option | Description |
|--------|-------------|
| `--older-than <age>` | Minimum age to remove (`s`, `m`, `h`, or `d`; default: `3d`) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BOX_STRATEGY` | Default workspace strategy (`worktree`, `clone`, or `layer`). Overridden by `--strategy` |
| `BOX_UPDATE` | Whether `box workspace add` fetches sources first (`yes` by default; `no` to skip). Overridden by `--update` / `--no-update` |
| `BOX_LAYER_REFRESH` | How the `layer` strategy rolls base images forward after creating a workspace: `background` (default), `sync`, or `off` |
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

- Tab completion for workspaces, sources, and presets
- A `box` shell function so `box workspace switch` / `box workspace add` change your working directory

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

### Layer (macOS)

```
~/.box/layers/frontend/bases/<commit>.sparseimage   (checkout already materialized)
        │ clonefile (copy-on-write)
        ▼
~/.box/workspaces/my-feature/.box-images/frontend.sparseimage
        │ hdiutil attach
        ▼
~/.box/workspaces/my-feature/frontend/   (linked worktree of frontend.git)
```

Each repo is a per-workspace APFS sparse disk image cloned from a per-repo *base* image that already contains a checkout. Creating a workspace clones one file and mounts it instead of writing every tracked file, so it stays fast for large repos and the first `git status` is warm. Like `worktree`, it shares the bare repo's objects and refs.

- The first `layer` workspace for a repo builds its base image (slower than a normal checkout).
- When `origin` has moved, the new workspace is updated to the latest commit and the base is rolled forward in the background (`BOX_LAYER_REFRESH`).
- `post-checkout` hooks run in the final workspace path, like `git worktree add`.
- Images are not remounted automatically after a reboot; `box workspace switch` re-attaches them.
- Workspace images grow with writes (build output included) and do not shrink when files are deleted. Disk space is shared with the host volume, so keep free space available.

### Comparison

| | Worktree | Clone | Layer (macOS) |
|---|---|---|---|
| Speed | Instant (small repos) | Fast (hardlinks) | Fastest for large repos |
| Disk usage | Minimal (shared objects) | Low (hardlinked objects) | Copy-on-write; grows with changes |
| Isolation | Separate working tree, shared `.git` | Fully independent `.git` | Separate volume per repo, shared `.git` |
| Best for | Feature branches, quick experiments | Full isolation, destructive operations | Large repos, frequent workspace creation |

| Aspect | Detail |
|--------|--------|
| Bare repos | `~/.box/repos/<name>.git/` |
| Workspace location | `~/.box/workspaces/<session>/` |
| Session metadata | `~/.box/sessions/<session>/` |
| Presets | `~/.box/presets/<name>` |
| Layer base images | `~/.box/layers/<name>/` |
| Default strategy | `git worktree` (override with `--strategy clone` or `--strategy layer`) |
| Cleanup | `box workspace remove` deletes workspace and session data |

## License

MIT
