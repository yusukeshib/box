# box

[日本語](README.ja.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

Sandboxed git workspaces for development. Clone, branch, break things — your repo stays untouched.

![demo](./demo.gif)

## Why box?

Box gives you **isolated** git workspaces with **session tracking** — so you can freely experiment without touching your original repository.

Each session gets its own workspace. By default, `git clone --local` creates a fully independent repo with hardlinks — fast even for large repos, and nothing you do can affect the original. Alternatively, `--strategy worktree` uses `git worktree` for even faster, space-efficient workspaces that share the object store.

## Features

- **Isolated git workspaces** — `git clone --local` (default) or `git worktree` for per-session workspaces; host files are never modified
- **Session tracking** — metadata tracks project directory, command, strategy, and creation time
- **Multi-repo workspaces** — register repos and create workspaces spanning multiple repos
- **Shell integration** — `cd` into workspaces automatically

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
box new my-feature
# Creates an isolated git workspace and cd's into it
```

Box must be run inside a git repository. It clones the current repo into `~/.box/workspaces/<name>/`.

```bash
# Work in the isolated workspace...
$ git checkout -b experiment
$ make test  # break things freely

# Done? Clean up
box remove my-feature
```

Running `box` with no arguments launches the interactive TUI. If sessions exist, you can select one to resume; otherwise it prompts to create one.

## Session Naming

Each session gets a simple name:

```bash
box new my-feature
box new my-feature -- make test
```

## Usage

```bash
box                                    Interactive TUI session manager
box new [name] [options] [-- cmd...]   Create a new session
box exec <name> -- <cmd...>            Run a command in a session
box list [options]                     List sessions (alias: ls)
box remove <name>                      Remove a session or workspace
box cd <name>                          Print host project directory
box repo add [path]                    Register a git repo
box repo remove <name>                 Unregister a repo
box repo list                          List registered repos (alias: ls)
box config zsh|bash                    Output shell completions
box upgrade                            Upgrade to latest version
```

### Create a session

```bash
# Interactive prompt (asks for name, command)
box new

# With a specific command
box new my-feature -- make test

# Multi-repo workspace (select specific repos)
box new my-feature --repo app-a --repo app-b

# Use git worktree instead of clone (faster, shares object store)
box new my-feature --strategy worktree
BOX_STRATEGY=worktree box new my-feature
```

### List and manage sessions

```bash
box list                        # List all sessions
box ls                          # Alias
box list -q                     # Names only (for scripting)
box list -p                     # Only sessions for the current project
box remove my-feature           # Remove session, workspace, and data
```

### Navigate between workspaces

```bash
box cd my-feature               # Print the host project directory
```

## Options

### `box new`

| Option | Description |
|--------|-------------|
| `--strategy <strategy>` | Workspace strategy: `clone` (default) or `worktree`. Overrides `$BOX_STRATEGY` |
| `--repo <name>` | Select specific repos (repeatable). Only with registered repos |
| `-- cmd...` | Command to run (default: `$BOX_DEFAULT_CMD` if set) |

### `box list`

| Option | Description |
|--------|-------------|
| `--project`, `-p` | Show only sessions for the current project directory |
| `--quiet`, `-q` | Only print session names (useful for scripting) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BOX_DEFAULT_CMD` | Default command for new sessions, used when no `-- cmd` is provided |
| `BOX_STRATEGY` | Workspace strategy: `clone` (default) or `worktree` |

## Shell Completions

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

## How It Works

```
your-repo/          box new my-feature            ~/.box/workspaces/my-feature/
  .git/        ──── git clone --local ────>         .git/  (independent)
  src/                                              src/   (hardlinked)
  ...                                               ...
```

By default, `git clone --local` creates a fully independent git repo using hardlinks for file objects. The clone has its own `.git` directory — commits, branches, resets, and destructive operations in the workspace cannot affect your original repository.

With `--strategy worktree`, box uses `git worktree add --detach` instead. This shares the object store with the parent repo, making workspace creation faster and more space-efficient. The tradeoff is that worktrees share refs with the parent — use this when you want lightweight workspaces and don't need full git isolation.

| Aspect | Detail |
|--------|--------|
| Workspace location | `~/.box/workspaces/<name>/` |
| Session metadata | `~/.box/sessions/<name>/` |
| Git isolation | Full with `clone` (default); shared object store with `worktree` |
| Cleanup | `box remove` deletes workspace and session |

## Design Decisions

<details>
<summary><strong>Why <code>git clone --local</code> as the default?</strong></summary>

| Strategy | Trade-off |
|----------|-----------|
| **Bind-mount the host repo** | No isolation at all; modifications affect your actual files |
| **git worktree** | Shares the `.git` directory with the host; checkout, reset, and rebase can affect host branches and refs |
| **Bare-git mount** | Still shares state; branch creates/deletes affect the host |
| **Branch-only isolation** | Nothing stops destructive git commands on shared refs |
| **Full copy (`cp -r`)** | Truly isolated but slow for large repos |

`git clone --local` is fully independent (own `.git`), fast (hardlinks), complete (full history), and simple (no wrapper scripts).

That said, `git worktree` is available via `--strategy worktree` for cases where speed and disk savings matter more than full isolation.

</details>

## Claude Code Integration

Box works well with [Claude Code](https://docs.anthropic.com/en/docs/claude-code) for running AI agents in isolated workspaces:

```bash
box new ai-experiment -- claude
```

Everything the agent does stays in the workspace. Delete the session when you're done.

## License

MIT
