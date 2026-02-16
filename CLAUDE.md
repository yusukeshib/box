# Project: box

Sandboxed Docker environments for git repos. Written in Rust.

## Setup

- `git config core.hooksPath .githooks` — enable pre-commit hook (runs fmt + clippy)

## Build & Test

- `cargo fmt` — always run before committing
- `cargo clippy` — fix any warnings before committing
- `cargo build` — verify compilation
- `cargo test` — run all tests

## Conventions

- Shell completions (zsh + bash) are inline in `src/main.rs` — update both when adding CLI flags
- Use raw ANSI escape codes (`\x1b[2m` for dim, `\x1b[0m` for reset) for styled CLI output
- Session metadata is stored as flat files under `~/.box/sessions/<name>/`
- Workspace git clones live under `~/.box/workspaces/<name>/`
- `project_dir` is always resolved to the git root via `git::find_root()`
