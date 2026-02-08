# realm

Run Claude Code with `--dangerously-skip-permissions` safely in ephemeral Docker containers.

## Features

- **Named sessions** - Create, list, apply, and remove sessions by name
- **Ephemeral containers** - Auto-destroyed after each session (`--rm`)
- **Full interactive TUI** - Arrow keys, Ctrl+C, multiline input all work
- **Read-only project mount** - Your project is never modified directly
- **Patch-based workflow** - Changes are extracted as a git patch for review before applying
- **Flexible authentication** - Supports both API key and shared credentials
- **Persistent history** - Conversation history saved via `~/.claude` mount
- **Sandboxed execution** - Only your project directory is accessible (read-only)

## Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/yusukeshib/realm/main/install.sh | bash
```

This will:
1. Download files to `~/.realm`
2. Build the Docker image
3. Symlink `realm` to `~/.local/bin`

Ensure `~/.local/bin` is in your PATH. Add to `~/.zshrc` or `~/.bashrc` if needed:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Authentication

Realm supports two authentication methods:

### Option 1: API Key (Recommended for macOS)

Set your Anthropic API key in your shell configuration file:

```bash
# Add to ~/.zshrc or ~/.bashrc:
export ANTHROPIC_API_KEY="sk-ant-..."
```

Then reload your shell:
```bash
source ~/.zshrc  # or source ~/.bashrc
```

Get your API key from: https://console.anthropic.com

**Note**: On macOS, this is the preferred method because Claude Code stores credentials in the macOS Keychain, which Docker containers cannot access.

### Option 2: Shared ~/.claude Directory (Linux)

On Linux systems, realm automatically shares your host's `~/.claude` credentials directory with the container (no extra setup needed).

## Usage

```bash
realm new <name> [--remote] [project_dir] [claude args...]
realm resume <name> [claude args...]              # Resume existing session
realm list                                         # List all sessions
realm push <name>                                  # Push changes to branch
realm remove <name>                                # Delete session
```

### Start a session

```bash
# From within a project directory (defaults to current dir)
realm new my-feature

# Specify a project directory explicitly
realm new my-feature ~/projects/my-app

# Resume from an existing remote branch
realm new my-feature --remote

# Pass extra arguments to Claude
realm new quick-fix "explain this codebase"
```

With `--remote`, the session fetches `origin/<name>` and starts with that branch's changes pre-applied (diffed against `main`). The container always works on top of `main`.

### Resume a session

```bash
# Continue working on an existing session
realm resume my-feature

# Resume with specific instructions
realm resume my-feature "add more tests for the authentication module"
```

When you resume a session:
1. Previous changes from `changes.patch` are promoted to `base.patch`
2. The container starts with all accumulated changes applied
3. New changes are captured as a fresh `changes.patch`
4. You can resume multiple times — changes accumulate in `base.patch`
5. `realm push` always pushes the combined changes

This allows for iterative development: make changes, exit, resume, make more changes, and finally push all accumulated work to a branch.

### List sessions

```bash
realm list
```

```
NAME                 PROJECT                                  CREATED                PATCH
----                 -------                                  -------                -----
my-feature           /Users/you/projects/my-app               2026-02-07 12:00:00 UTC yes (4KB)
quick-fix            /Users/you/projects/other                 2026-02-07 12:30:00 UTC no
```

### Push changes

```bash
# Push changes to a branch named after the session
realm push my-feature
```

This applies the patch in a temporary worktree, commits, and force-pushes to `origin/my-feature`. Your local branches and working directory are never touched.

### Remove a session

```bash
realm remove my-feature
```

## Session Storage

Sessions are stored in `~/.realm/sessions/{name}/` with:
- `changes.patch` — Git diff of current session changes (only if changes were made)
- `base.patch` — Accumulated changes from previous sessions (after using `realm resume`)
- `project_dir` — Absolute path to the original project
- `created_at` — Timestamp of session creation
- `resumed_at` — Timestamp of last resume (if session was resumed)

## Environment Variables

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | Passed to container if set (otherwise uses `~/.claude` auth) |
| `REALM_DOCKER_ARGS` | Extra Docker flags (e.g., `--network host`, additional `-v` mounts) |

Example:

```bash
REALM_DOCKER_ARGS="--network host -v /data:/data:ro" realm new my-session ~/projects/my-app
```

## Version Pinning

Pin a specific Claude Code version at build time:

```bash
docker build --build-arg CLAUDE_CODE_VERSION=1.0.5 -t realm:latest ~/.realm/
```

## Security Model

| Aspect | Protection |
|--------|------------|
| File system | Project mounted read-only; changes extracted as patch |
| Host system | Fully isolated from container; no direct writes to host files |
| Container | Destroyed after each exit |
| Auth | Shared from host `~/.claude` (no env vars needed) |
| History | Persists in `~/.claude` on host |
| Changes | Require explicit `realm push` to create branch and push to remote |

## Troubleshooting

For common issues and solutions, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).

## License

MIT
