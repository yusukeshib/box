mod config;
mod git;
mod repo;
mod session;
#[cfg(test)]
mod test_util;
mod tui;
mod workspace;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "box",
    about = "Sandboxed git workspaces for development",
    after_help = "Examples:\n  box                                         # interactive session manager\n  box new my-feature --repo app-a              # create a new session\n  box new my-feature --repo app-a --repo app-b # select specific repos\n  box new my-feature --repo app --strategy worktree # use git worktree\n  box edit my-feature                          # add/remove repos in a session\n  box list                                     # list all sessions\n  box remove                                   # interactive session removal\n  box remove my-feature                        # remove a session by name\n  box switch my-feature                        # switch to a session\n  box repo add .                               # register current dir as a repo\n  box repo list                                # list registered repos\n  box repo remove my-app                       # unregister a repo\n  box repo update                              # fetch registered repos\n  box upgrade                                  # self-update"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new session
    New(CreateArgs),
    /// Edit repos in an existing session
    Edit(EditArgs),
    /// Remove a session
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    /// List sessions
    #[command(alias = "ls")]
    List(ListArgs),
    /// Switch to a session
    #[command(alias = "cd", alias = "sw")]
    Switch {
        /// Session name
        name: String,
    },
    /// Manage registered repos
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Self-update to the latest version
    Upgrade,
    /// Output shell configuration (e.g. eval "$(box config zsh)")
    Config {
        #[command(subcommand)]
        shell: ConfigShell,
    },
}

#[derive(Subcommand, Debug)]
enum RepoAction {
    /// Register a git repo
    Add {
        /// Path to the repo (defaults to current directory)
        path: Option<String>,
    },
    /// Unregister a repo by name
    #[command(alias = "rm")]
    Remove {
        /// Repo name
        name: String,
    },
    /// List registered repos
    #[command(alias = "ls")]
    List,
    /// Fetch all refs for registered repos
    Update(PullArgs),
}

#[derive(clap::Args, Debug)]
struct CreateArgs {
    /// Session name
    name: String,

    /// Select specific repos by name (can be repeated)
    #[arg(long, required = true)]
    repo: Vec<String>,

    /// Workspace strategy: worktree (default) or clone
    #[arg(long, env = "BOX_STRATEGY", default_value = "worktree")]
    strategy: String,
}

#[derive(clap::Args, Debug)]
struct EditArgs {
    /// Session name
    name: String,
}

#[derive(clap::Args, Debug)]
struct RemoveArgs {
    /// Session name (opens interactive selector if omitted)
    name: Option<String>,
}

#[derive(clap::Args, Debug)]
struct PullArgs {
    /// Fetch all registered repos without interactive selection
    #[arg(long, short)]
    all: bool,
}

#[derive(clap::Args, Debug)]
struct ListArgs {
    /// Show only sessions for the current project directory
    #[arg(long, short)]
    project: bool,
    /// Only print session names
    #[arg(long, short)]
    quiet: bool,
}

#[derive(Subcommand, Debug)]
enum ConfigShell {
    /// Output Zsh completions
    Zsh,
    /// Output Bash completions
    Bash,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::New(args)) => {
            if std::env::var_os("BOX_SESSION").is_some() {
                eprintln!(
                    "Error: cannot nest box sessions (already inside session {:?})",
                    std::env::var("BOX_SESSION").unwrap_or_default()
                );
                std::process::exit(1);
            }
            if let Err(e) = update_repos(&args.repo) {
                eprintln!("Warning: repo update failed: {}", e);
            }
            let strategy = match workspace::Strategy::from_str(&args.strategy) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };
            cmd_create(&args.name, args.repo, strategy)
        }
        Some(Commands::Edit(args)) => cmd_edit(&args.name),
        Some(Commands::Remove(args)) => match &args.name {
            Some(name) => cmd_remove(name),
            None => cmd_remove_tui(),
        },
        Some(Commands::List(args)) => cmd_list_sessions(&args),
        Some(Commands::Switch { name }) => cmd_cd(&name),
        Some(Commands::Repo { action }) => match action {
            RepoAction::Add { path } => cmd_repo_add(path),
            RepoAction::Remove { name } => cmd_repo_remove(&name),
            RepoAction::List => cmd_repo_list(),
            RepoAction::Update(args) => cmd_pull(&args),
        },
        Some(Commands::Upgrade) => cmd_upgrade(),
        Some(Commands::Config { shell }) => match shell {
            ConfigShell::Zsh => cmd_config_zsh(),
            ConfigShell::Bash => cmd_config_bash(),
        },
        None => cmd_default(),
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn output_cd_path(path: &str) {
    if let Ok(cd_file) = std::env::var("BOX_CD_FILE") {
        let _ = fs::write(cd_file, path);
    } else {
        println!("{}", path);
    }
}

fn rename_terminal_tab(name: &str) {
    if std::env::var_os("ZELLIJ").is_some() {
        let _ = std::process::Command::new("zellij")
            .args(["action", "rename-tab", name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    } else if std::env::var_os("TMUX").is_some() {
        let _ = std::process::Command::new("tmux")
            .args(["rename-window", name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Resolve the workspace path for a session. For single-repo sessions,
/// returns the repo subdirectory; for multi-repo, returns the workspace root.
fn resolve_workspace_path(name: &str) -> Result<PathBuf> {
    let workspace_root = config::box_root()?.join("workspaces").join(name);
    let sess = session::load(name)?;
    if sess.repos.len() == 1 {
        Ok(workspace_root.join(&sess.repos[0]))
    } else {
        Ok(workspace_root)
    }
}

/// Shorten a project path for display by abbreviating intermediate components
/// to their first character. e.g. `/Users/yusuke/projects/my-app` => `/U/y/p/my-app`
/// The home directory prefix is replaced with `~` first.
pub(crate) fn shorten_project_path(path: &str, home: &str) -> String {
    let (prefix, rest) = if !home.is_empty() {
        if let Some(r) = path.strip_prefix(home) {
            ("~", r)
        } else {
            ("", path)
        }
    } else {
        ("", path)
    };

    let full = format!("{}{}", prefix, rest);
    let parts: Vec<&str> = full.split('/').collect();

    if parts.len() <= 2 {
        return full;
    }

    // Abbreviate all components except the first (empty for leading /) and last
    let last = parts.len() - 1;
    let shortened: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            if i == 0 || i == last || part.is_empty() {
                part.to_string()
            } else {
                part.chars()
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            }
        })
        .collect();

    shortened.join("/")
}

/// Resolve the current directory to a project_dir suitable for filtering sessions.
///
/// 1. If the cwd is inside a workspace (`~/.box/workspaces/<name>/`), look up
///    that session's project_dir so we can find sibling sessions for the same project.
/// 2. Otherwise, walk up to the nearest git root and use that.
fn resolve_project_dir(
    cwd: &std::path::Path,
    sessions: &[session::SessionSummary],
) -> Option<String> {
    // Check if we're inside a workspace directory
    if let Ok(root) = config::box_root() {
        let workspaces = root.join("workspaces");
        if let Ok(workspaces) = std::fs::canonicalize(&workspaces) {
            if cwd.starts_with(&workspaces) {
                // Extract the workspace name (first component after workspaces/)
                if let Some(ws_name) = cwd.strip_prefix(&workspaces).ok().and_then(|r| {
                    r.components()
                        .next()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                }) {
                    // Find session with this name to get its project_dir
                    if let Some(s) = sessions.iter().find(|s| s.name == ws_name) {
                        if !s.project_dir.is_empty() {
                            return Some(s.project_dir.clone());
                        }
                    }
                }
            }
        }
    }

    // Fall back to git root
    git::find_root(cwd).map(|r| r.to_string_lossy().to_string())
}

/// `box` with no args: interactive TUI to create a session.
fn cmd_default() -> Result<i32> {
    if std::env::var_os("BOX_SESSION").is_some() {
        bail!(
            "Cannot nest box sessions (already inside session {:?}).",
            std::env::var("BOX_SESSION").unwrap_or_default()
        );
    }
    cmd_create_tui()
}

/// `box create` with no name: prompt for session details.
fn cmd_create_tui() -> Result<i32> {
    let strategy = workspace::Strategy::resolve(None)?;
    match tui::create_session()? {
        tui::TuiAction::New { name, repos } => {
            if let Err(e) = update_repos(&repos) {
                eprintln!("Warning: repo update failed: {}", e);
            }
            cmd_create(&name, repos, strategy)
        }
        _ => Ok(0),
    }
}

fn cmd_list_sessions(args: &ListArgs) -> Result<i32> {
    let mut sessions = session::list()?;

    if args.project {
        let cwd = std::env::current_dir()?;
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let project = resolve_project_dir(&cwd, &sessions);
        if let Some(project) = project {
            sessions.retain(|s| s.project_dir == project);
        } else {
            sessions.clear();
        }
    }

    if args.quiet {
        for s in &sessions {
            println!("{}", s.name);
        }
        return Ok(0);
    }

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(0);
    }

    let home = config::home_dir().unwrap_or_default();

    // Compute column widths
    let name_w = sessions
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0)
        .max(4);

    let shorten_path = |p: &str| -> String { shorten_project_path(p, &home) };

    let project_display = |s: &session::SessionSummary| -> String {
        if !s.repos.is_empty() {
            s.repos.join(", ")
        } else {
            shorten_path(&s.project_dir)
        }
    };

    let project_w = sessions
        .iter()
        .map(|s| project_display(s).len())
        .max()
        .unwrap_or(0)
        .max(7);
    let strat_w = sessions
        .iter()
        .map(|s| s.strategy.len())
        .max()
        .unwrap_or(0)
        .max(5);

    println!(
        "\x1b[2m  {:<name_w$}  {:<project_w$}  {:<strat_w$}  CREATED\x1b[0m",
        "NAME", "PROJECT", "STRAT",
    );

    for s in &sessions {
        let project = project_display(s);
        println!(
            "  {:<name_w$}  {:<project_w$}  {:<strat_w$}  {}",
            s.name, project, s.strategy, s.created_at,
        );
    }

    Ok(0)
}

fn cmd_create(name: &str, repo_names: Vec<String>, strategy: workspace::Strategy) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();

    if session::session_exists(name)? {
        bail!("Session '{}' already exists.", name);
    }

    // Resolve repos
    let all_repos = repo::list()?;
    let selected_repos: Vec<repo::RepoEntry> = {
        let mut result = Vec::new();
        for n in &repo_names {
            let entry = all_repos
                .iter()
                .find(|r| r.name == *n)
                .ok_or_else(|| anyhow::anyhow!("Repo '{}' not found in registry.", n))?;
            result.push(entry.clone());
        }
        result
    };

    if selected_repos.is_empty() {
        bail!("No repos registered. Run `box repo add <path>` first.");
    }

    let repo_names_list: Vec<String> = selected_repos.iter().map(|r| r.name.clone()).collect();

    // Resolve config (project_dir is empty for multi-repo sessions)
    let cfg = config::resolve(config::BoxConfigInput {
        name: name.to_string(),
        project_dir: String::new(),
        env: vec![],
        repos: repo_names_list,
    })?;

    eprintln!("\x1b[2msession:\x1b[0m {}", name);
    eprintln!("\x1b[2mrepos:\x1b[0m {}", cfg.repos.join(", "));
    eprintln!("\x1b[2mstrategy:\x1b[0m {}", strategy);
    eprintln!();

    let mut sess = session::Session::from(cfg);
    sess.strategy = strategy.as_str().to_string();
    session::save(&sess)?;

    let workspace_path = workspace::ensure_workspace(name, &selected_repos, strategy)?;
    if selected_repos.len() == 1 {
        let repo_path = Path::new(&workspace_path).join(&selected_repos[0].name);
        output_cd_path(&repo_path.to_string_lossy());
    } else {
        output_cd_path(&workspace_path);
    }
    rename_terminal_tab(name);

    Ok(0)
}

fn cmd_edit(name: &str) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let sess = session::load(name)?;
    let strategy = workspace::Strategy::from_str(&sess.strategy)?;
    let current_repos = &sess.repos;

    match tui::edit_session(current_repos)? {
        tui::TuiAction::Edit { repos: new_repos } => {
            let all_repos = repo::list()?;

            // Determine added and removed repos
            let added: Vec<&str> = new_repos
                .iter()
                .filter(|r| !current_repos.contains(r))
                .map(|r| r.as_str())
                .collect();
            let removed: Vec<&str> = current_repos
                .iter()
                .filter(|r| !new_repos.contains(r))
                .map(|r| r.as_str())
                .collect();

            // Add newly added repos using the session's strategy
            if !added.is_empty() {
                let repos_to_add: Vec<repo::RepoEntry> = added
                    .iter()
                    .filter_map(|name| all_repos.iter().find(|r| r.name == *name).cloned())
                    .collect();
                workspace::ensure_workspace(name, &repos_to_add, strategy)?;
            }

            // Remove workspace directories for removed repos
            for repo_name in &removed {
                workspace::remove_repo_by_strategy(name, repo_name, strategy);
            }

            // Update session metadata
            session::update_repos(name, &new_repos)?;

            if !added.is_empty() {
                eprintln!("\x1b[2madded:\x1b[0m {}", added.join(", "));
            }
            if !removed.is_empty() {
                eprintln!("\x1b[2mremoved:\x1b[0m {}", removed.join(", "));
            }
            if added.is_empty() && removed.is_empty() {
                eprintln!("No changes.");
            }

            Ok(0)
        }
        _ => Ok(0),
    }
}

fn cmd_remove(name: &str) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let sess = session::load(name)?;
    let strategy =
        workspace::Strategy::from_str(&sess.strategy).unwrap_or(workspace::Strategy::Clone);

    workspace::remove_workspace_by_strategy(name, &sess.repos, strategy);
    session::remove_dir(name)?;

    if !sess.project_dir.is_empty() {
        output_cd_path(&sess.project_dir);
    }
    println!("Session '{}' removed.", name);
    Ok(0)
}

fn cmd_remove_tui() -> Result<i32> {
    match tui::select_sessions()? {
        tui::TuiAction::Remove { sessions } => {
            for name in &sessions {
                if let Ok(sess) = session::load(name) {
                    let strategy = workspace::Strategy::from_str(&sess.strategy)
                        .unwrap_or(workspace::Strategy::Clone);
                    workspace::remove_workspace_by_strategy(name, &sess.repos, strategy);
                } else {
                    workspace::remove_workspace(name);
                }
                session::remove_dir(name)?;
                eprintln!("Session '{}' removed.", name);
            }
            Ok(0)
        }
        _ => Ok(0),
    }
}

fn cmd_cd(name: &str) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();
    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }
    let path = resolve_workspace_path(name)?;
    output_cd_path(&path.to_string_lossy());
    rename_terminal_tab(name);
    Ok(0)
}

fn cmd_repo_add(path: Option<String>) -> Result<i32> {
    let path = path.unwrap_or_else(|| ".".to_string());
    repo::add(&path)?;
    Ok(0)
}

fn cmd_repo_remove(name: &str) -> Result<i32> {
    repo::remove(name)?;
    Ok(0)
}

fn cmd_repo_list() -> Result<i32> {
    let repos = repo::list()?;
    if repos.is_empty() {
        println!("No repos registered.");
        return Ok(0);
    }

    let name_w = repos.iter().map(|r| r.name.len()).max().unwrap_or(0).max(4);
    let url_display: Vec<String> = repos
        .iter()
        .map(|r| repo::origin_url(&r.path).unwrap_or_else(|| "(local)".to_string()))
        .collect();
    let url_w = url_display
        .iter()
        .map(|u| u.len())
        .max()
        .unwrap_or(0)
        .max(6);

    println!("\x1b[2m  {:<name_w$}  {:<url_w$}\x1b[0m", "NAME", "ORIGIN");

    for (r, u) in repos.iter().zip(&url_display) {
        println!("  {:<name_w$}  {:<url_w$}", r.name, u);
    }
    Ok(0)
}

fn cmd_config_zsh() -> Result<i32> {
    print!(
        r#"__box_sessions() {{
    local -a sessions
    local __box_root="${{BOX_ROOT:-$HOME/.box}}"
    if [[ -d "$__box_root/sessions" ]]; then
        for sess in "$__box_root/sessions"/*(N/); do
            if [[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]; then
                local sess_name=${{sess:t}}
                local desc=""
                if [[ -f "$sess/project_dir" ]]; then
                    desc=$(< "$sess/project_dir")
                    desc=${{desc/#$HOME/\~}}
                fi
                if [[ -n "$desc" ]]; then
                    sessions+=("$sess_name:$desc")
                else
                    sessions+=("$sess_name")
                fi
            fi
        done
    fi
    if (( ${{#sessions}} )); then
        _describe 'session' sessions
    fi
}}

__box_repos() {{
    local -a repos
    local __box_root="${{BOX_ROOT:-$HOME/.box}}"
    if [[ -d "$__box_root/repos" ]]; then
        for bare in "$__box_root/repos"/*.git(N/); do
            local name=${{bare:t}}
            name=${{name%.git}}
            [[ -n "$name" ]] && repos+=("$name")
        done
    fi
    if (( ${{#repos}} )); then
        _describe 'repo' repos
    fi
}}

_box() {{
    local curcontext="$curcontext" state line
    typeset -A opt_args

    _arguments -C \
        '1: :->subcmd' \
        '*:: :->args'

    case $state in
        subcmd)
            local -a subcmds
            subcmds=(
                'new:Create a new session'
                'edit:Edit repos in an existing session'
                'remove:Remove a session'
                'rm:Remove a session'
                'list:List sessions'
                'switch:Switch to a session'
                'sw:Switch to a session'
                'cd:Switch to a session'
                'repo:Manage registered repos'
                'upgrade:Self-update to the latest version'
                'config:Output shell configuration'
            )
            _describe 'subcommand' subcmds
            ;;
        args)
            case $words[1] in
                new)
                    _arguments \
                        '*--repo=[Select specific repo]:repo:__box_repos' \
                        '--strategy=[Workspace strategy]:strategy:(clone worktree)' \
                        '1:session name:' \
                        '*:command:'
                    ;;
                list|ls)
                    _arguments \
                        '--project[Show only sessions for the current project]' \
                        '-p[Show only sessions for the current project]' \
                        '--quiet[Only print session names]' \
                        '-q[Only print session names]'
                    ;;
                remove|rm)
                    if (( CURRENT == 2 )); then
                        __box_sessions
                    fi
                    ;;
                edit|switch|sw|cd)
                    if (( CURRENT == 2 )); then
                        __box_sessions
                    fi
                    ;;
                repo)
                    if (( CURRENT == 2 )); then
                        local -a repo_subcmds
                        repo_subcmds=('add:Register a git repo' 'remove:Unregister a repo' 'rm:Unregister a repo' 'list:List registered repos' 'ls:List registered repos' 'update:Fetch all refs')
                        _describe 'repo subcommand' repo_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            remove|rm)
                                __box_repos
                                ;;
                            add)
                                _files -/
                                ;;
                            update)
                                _arguments \
                                    '--all[Fetch all registered repos]' \
                                    '-a[Fetch all registered repos]'
                                ;;
                        esac
                    fi
                    ;;
                config)
                    if (( CURRENT == 2 )); then
                        local -a shells
                        shells=('zsh:Zsh completion script' 'bash:Bash completion script')
                        _describe 'shell' shells
                    fi
                    ;;
            esac
            ;;
    esac
}}
compdef _box box

box() {{
    local __box_cd_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    rm -f "$__box_cd_file"
    return $__box_exit
}}
"#
    );
    Ok(0)
}

fn cmd_config_bash() -> Result<i32> {
    print!(
        r#"_box() {{
    local cur prev words cword
    _init_completion || return

    local subcommands="new edit remove rm list switch sw cd repo upgrade config"
    local session_cmds="edit remove rm switch sw cd"
    local __box_root="${{BOX_ROOT:-$HOME/.box}}"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        return
    fi

    local subcmd="${{words[1]}}"
    [[ -z "$subcmd" ]] && return

    case "$subcmd" in
        new)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--repo --strategy" -- "$cur"))
                    ;;
            esac
            if [[ "$prev" == "--strategy" ]]; then
                COMPREPLY=($(compgen -W "clone worktree" -- "$cur"))
            fi
            ;;
        list|ls)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--project -p --quiet -q" -- "$cur"))
                    ;;
            esac
            ;;
        edit|remove|rm|switch|sw|cd)
            if [[ $cword -eq 2 ]]; then
                local sessions=""
                if [[ -d "$__box_root/sessions" ]]; then
                    for sess in "$__box_root/sessions"/*/; do
                        ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                    done
                fi
                COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
            fi
            ;;
        repo)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add remove rm list ls update" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${{words[2]}}" in
                    remove|rm)
                        local repos=""
                        if [[ -d "$__box_root/repos" ]]; then
                            for bare in "$__box_root/repos"/*.git; do
                                [[ -d "$bare" ]] || continue
                                local name=$(basename "$bare" .git)
                                [[ -n "$name" ]] && repos+=" $name"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$repos" -- "$cur"))
                        ;;
                    add)
                        COMPREPLY=($(compgen -d -- "$cur"))
                        ;;
                    update)
                        case "$cur" in
                            -*)
                                COMPREPLY=($(compgen -W "--all -a" -- "$cur"))
                                ;;
                        esac
                        ;;
                esac
            fi
            ;;
        config)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "zsh bash" -- "$cur"))
            fi
            ;;
    esac
}}
complete -F _box box

box() {{
    local __box_cd_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    rm -f "$__box_cd_file"
    return $__box_exit
}}
"#
    );
    Ok(0)
}

/// Fetch all refs for a bare repo.
fn update_repo(entry: &repo::RepoEntry) -> Result<bool> {
    let status = std::process::Command::new("git")
        .args(["-C", &entry.path, "fetch", "--all", "--prune"])
        .status()?;
    if !status.success() {
        eprintln!("  \x1b[31mfetch failed\x1b[0m");
        return Ok(false);
    }
    Ok(true)
}

/// Fetch & pull only the named repos (used by --update flag).
fn update_repos(names: &[String]) -> Result<()> {
    let all_repos = repo::list()?;
    let selected: Vec<&repo::RepoEntry> = all_repos
        .iter()
        .filter(|r| names.contains(&r.name))
        .collect();
    if selected.is_empty() {
        return Ok(());
    }

    eprintln!("\x1b[2mUpdating repos…\x1b[0m");
    for entry in &selected {
        eprintln!("\x1b[1m{}\x1b[0m", entry.name);
        update_repo(entry)?;
        eprintln!();
    }

    Ok(())
}

fn cmd_pull(args: &PullArgs) -> Result<i32> {
    let all_repos = repo::list()?;

    let selected = if args.all {
        if all_repos.is_empty() {
            eprintln!("No repos registered. Use `box repo add` to register a repo.");
            return Ok(1);
        }
        all_repos.iter().map(|r| r.name.clone()).collect()
    } else {
        match tui::select_repos("Select repos to fetch (Space=toggle, Enter=confirm):")? {
            Some(repos) => repos,
            None => return Ok(0),
        }
    };

    for name in &selected {
        let entry = all_repos
            .iter()
            .find(|r| &r.name == name)
            .ok_or_else(|| anyhow::anyhow!("Repo '{}' not found in registry.", name))?;

        eprintln!("\x1b[1m{}\x1b[0m", entry.name);
        update_repo(entry)?;
        eprintln!();
    }

    Ok(0)
}

fn cmd_upgrade() -> Result<i32> {
    let current_version = env!("CARGO_PKG_VERSION");
    println!("Current version: {}", current_version);

    println!("Checking for updates...");
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner("yusukeshib")
        .repo_name("box")
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build release list: {}", e))?
        .fetch()
        .map_err(|e| anyhow::anyhow!("Failed to fetch releases: {}", e))?;

    let latest = releases
        .first()
        .ok_or_else(|| anyhow::anyhow!("No releases found"))?;
    let latest_version = latest.version.trim_start_matches('v');

    println!("Latest version: {}", latest_version);

    if current_version == latest_version {
        println!("Already at latest version.");
        return Ok(0);
    }

    let asset_name = upgrade_asset_name()?;
    println!("Looking for asset: {}", asset_name);

    let asset_exists = latest.assets.iter().any(|a| a.name == asset_name);
    if !asset_exists {
        bail!(
            "Asset '{}' not found for this platform. Available assets: {}",
            asset_name,
            latest
                .assets
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let download_url = format!(
        "https://github.com/yusukeshib/box/releases/download/v{}/{}",
        latest_version, asset_name
    );

    println!("Downloading new version...");
    let tmp_path = upgrade_download(&download_url)?;
    let _guard = UpgradeTempGuard(tmp_path.clone());

    println!("Installing update...");
    self_update::self_replace::self_replace(&tmp_path).map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("permission denied") {
            anyhow::anyhow!(
                "Permission denied. Try running with elevated privileges (e.g., sudo box upgrade)."
            )
        } else {
            anyhow::anyhow!("{}", msg)
        }
    })?;

    println!("Upgraded from {} to {}.", current_version, latest_version);
    Ok(0)
}

/// RAII guard that removes the temp file on drop.
struct UpgradeTempGuard(std::path::PathBuf);

impl Drop for UpgradeTempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn upgrade_asset_name() -> Result<String> {
    let arch = std::env::consts::ARCH;
    let os_name = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => bail!("Unsupported platform: {}", other),
    };
    Ok(format!("box-{}-{}", arch, os_name))
}

fn upgrade_download(url: &str) -> Result<std::path::PathBuf> {
    let tmp_path = std::env::temp_dir().join(format!("box-update-{}", std::process::id()));
    let mut tmp_file = fs::File::create(&tmp_path)?;

    self_update::Download::from_url(url)
        .download_to(&mut tmp_file)
        .map_err(|e| anyhow::anyhow!("Download failed: {}", e))?;

    tmp_file.flush()?;
    drop(tmp_file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&tmp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp_path, perms)?;
    }

    Ok(tmp_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        let mut full_args = vec!["box"];
        full_args.extend_from_slice(args);
        Cli::try_parse_from(full_args).unwrap()
    }

    fn try_parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut full_args = vec!["box"];
        full_args.extend_from_slice(args);
        Cli::try_parse_from(full_args)
    }

    // -- No args = TUI --

    #[test]
    fn test_no_args_launches_tui() {
        let cli = parse(&[]);
        assert!(cli.command.is_none());
    }

    // -- new subcommand --

    #[test]
    fn test_new_name_only() {
        let cli = parse(&["new", "my-session", "--repo", "app"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.name, "my-session");
                assert_eq!(args.repo, vec!["app"]);
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_requires_name() {
        let result = try_parse(&["new"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_requires_repo() {
        let result = try_parse(&["new", "my-session"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_with_repo_flags() {
        let cli = parse(&["new", "my-session", "--repo", "app-a", "--repo", "app-b"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.name, "my-session");
                assert_eq!(args.repo, vec!["app-a", "app-b"]);
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_default_strategy() {
        let cli = parse(&["new", "my-session", "--repo", "app"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.strategy, "worktree");
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_with_strategy_clone() {
        let cli = parse(&["new", "my-session", "--repo", "app", "--strategy", "clone"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.strategy, "clone");
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    // -- edit subcommand --

    #[test]
    fn test_edit_parses() {
        let cli = parse(&["edit", "my-session"]);
        match cli.command {
            Some(Commands::Edit(args)) => {
                assert_eq!(args.name, "my-session");
            }
            other => panic!("expected Edit, got {:?}", other),
        }
    }

    #[test]
    fn test_edit_requires_name() {
        let result = try_parse(&["edit"]);
        assert!(result.is_err());
    }

    // -- remove subcommand --

    #[test]
    fn test_remove_parses() {
        let cli = parse(&["remove", "my-session"]);
        match cli.command {
            Some(Commands::Remove(args)) => {
                assert_eq!(args.name.as_deref(), Some("my-session"));
            }
            other => panic!("expected Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_alias_rm() {
        let cli = parse(&["rm", "my-session"]);
        match cli.command {
            Some(Commands::Remove(args)) => {
                assert_eq!(args.name.as_deref(), Some("my-session"));
            }
            other => panic!("expected Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_no_name_parses() {
        let cli = parse(&["remove"]);
        match cli.command {
            Some(Commands::Remove(args)) => {
                assert!(args.name.is_none());
            }
            other => panic!("expected Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_rejects_unknown_flags() {
        let result = try_parse(&["remove", "my-session", "-d"]);
        assert!(result.is_err());
    }

    // -- cd subcommand --

    #[test]
    fn test_switch_subcommand_parses() {
        let cli = parse(&["switch", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Switch { ref name }) if name == "my-session"
        ));
    }

    #[test]
    fn test_switch_alias_cd() {
        let cli = parse(&["cd", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Switch { ref name }) if name == "my-session"
        ));
    }

    #[test]
    fn test_switch_alias_sw() {
        let cli = parse(&["sw", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Switch { ref name }) if name == "my-session"
        ));
    }

    #[test]
    fn test_switch_requires_name() {
        let result = try_parse(&["switch"]);
        assert!(result.is_err());
    }

    // -- repo update subcommand --

    #[test]
    fn test_repo_update_subcommand_parses() {
        let cli = parse(&["repo", "update"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Update(args),
            }) => {
                assert!(!args.all);
            }
            other => panic!("expected Repo Update, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_update_all_flag() {
        let cli = parse(&["repo", "update", "--all"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Update(args),
            }) => {
                assert!(args.all);
            }
            other => panic!("expected Repo Update, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_update_all_short_flag() {
        let cli = parse(&["repo", "update", "-a"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Update(args),
            }) => {
                assert!(args.all);
            }
            other => panic!("expected Repo Update, got {:?}", other),
        }
    }

    // -- upgrade subcommand --

    #[test]
    fn test_upgrade_subcommand_parses() {
        let cli = parse(&["upgrade"]);
        assert!(matches!(cli.command, Some(Commands::Upgrade)));
    }

    #[test]
    fn test_upgrade_rejects_flags() {
        let result = try_parse(&["upgrade", "-d"]);
        assert!(result.is_err());
    }

    // -- config subcommand --

    #[test]
    fn test_config_zsh_subcommand_parses() {
        let cli = parse(&["config", "zsh"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                shell: ConfigShell::Zsh
            })
        ));
    }

    #[test]
    fn test_config_bash_subcommand_parses() {
        let cli = parse(&["config", "bash"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                shell: ConfigShell::Bash
            })
        ));
    }

    #[test]
    fn test_config_requires_shell() {
        let result = try_parse(&["config"]);
        assert!(result.is_err());
    }

    // -- list subcommand --

    #[test]
    fn test_list_no_flags() {
        let cli = parse(&["list"]);
        match cli.command {
            Some(Commands::List(args)) => {
                assert!(!args.quiet);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_list_quiet_flag() {
        let cli = parse(&["list", "-q"]);
        match cli.command {
            Some(Commands::List(args)) => {
                assert!(args.quiet);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_list_alias_ls() {
        let cli = parse(&["ls"]);
        match cli.command {
            Some(Commands::List(args)) => {
                assert!(!args.quiet);
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_list_rejects_positional_args() {
        let result = try_parse(&["list", "my-session"]);
        assert!(result.is_err());
    }

    // -- repo subcommand --

    #[test]
    fn test_repo_add_no_path() {
        let cli = parse(&["repo", "add"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Add { path },
            }) => {
                assert!(path.is_none());
            }
            other => panic!("expected Repo Add, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_add_with_path() {
        let cli = parse(&["repo", "add", "/tmp/my-repo"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Add { path },
            }) => {
                assert_eq!(path.as_deref(), Some("/tmp/my-repo"));
            }
            other => panic!("expected Repo Add, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_remove() {
        let cli = parse(&["repo", "remove", "my-app"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Remove { name },
            }) => {
                assert_eq!(name, "my-app");
            }
            other => panic!("expected Repo Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_remove_alias_rm() {
        let cli = parse(&["repo", "rm", "my-app"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Remove { name },
            }) => {
                assert_eq!(name, "my-app");
            }
            other => panic!("expected Repo Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_list() {
        let cli = parse(&["repo", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Repo {
                action: RepoAction::List
            })
        ));
    }

    // -- no --update flag (removed) --

    #[test]
    fn test_update_flag_rejected() {
        let result = try_parse(&["--update"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_update_flag_rejected() {
        let result = try_parse(&["new", "my-session", "--repo", "app", "--update"]);
        assert!(result.is_err());
    }

    // -- bare name is rejected (subcommand required) --

    #[test]
    fn test_bare_name_rejected() {
        let result = try_parse(&["my-session"]);
        assert!(result.is_err());
    }
}
