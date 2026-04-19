mod config;
mod git;
mod parallel;
mod preset;
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
    after_help = "Examples:\n  box                                         # interactive session manager\n  box new my-feature --repo app-a              # create a new session\n  box new my-feature --repo app-a --repo app-b # select specific repos\n  box new my-feature --preset work             # create session from preset\n  box new my-feature --repo app --strategy worktree # use git worktree\n  box edit my-feature                          # add/remove repos in a session\n  box list                                     # list all sessions\n  box remove                                   # interactive session removal\n  box remove my-feature                        # remove a session by name\n  box switch my-feature                        # switch to a session\n  box repo add .                               # register current dir as a repo\n  box repo list                                # list registered repos\n  box repo remove my-app                       # unregister a repo\n  box preset add work --repo app-a --repo app-b # define a preset\n  box preset edit work                          # edit repos in a preset\n  box preset list                               # list presets\n  box preset remove work                        # remove a preset\n  box upgrade                                  # self-update"
)]
struct Cli {
    /// Show detailed output
    #[arg(long, short = 'v', global = true, env = "BOX_VERBOSE")]
    verbose: bool,

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
    /// Manage session presets
    Preset {
        #[command(subcommand)]
        action: PresetAction,
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
}

#[derive(Subcommand, Debug)]
enum PresetAction {
    /// Create or update a preset
    Add {
        /// Preset name
        name: String,
        /// Repos to include (can be repeated; opens interactive selector if omitted)
        #[arg(long)]
        repo: Vec<String>,
    },
    /// Edit repos in an existing preset
    Edit {
        /// Preset name
        name: String,
    },
    /// Remove a preset
    #[command(alias = "rm")]
    Remove {
        /// Preset name
        name: String,
    },
    /// List presets
    #[command(alias = "ls")]
    List,
}

#[derive(clap::Args, Debug)]
struct CreateArgs {
    /// Session name
    name: String,

    /// Select specific repos by name (can be repeated)
    #[arg(long, group = "repo_source")]
    repo: Vec<String>,

    /// Use a preset (mutually exclusive with --repo)
    #[arg(long, group = "repo_source")]
    preset: Option<String>,

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

    let verbose = cli.verbose;

    let result = match cli.command {
        Some(Commands::New(args)) => cmd_new(args, verbose),
        Some(Commands::Edit(args)) => cmd_edit(&args.name, verbose),
        Some(Commands::Remove(args)) => match &args.name {
            Some(name) => cmd_remove(name, verbose),
            None => cmd_remove_tui(verbose),
        },
        Some(Commands::List(args)) => cmd_list_sessions(&args),
        Some(Commands::Switch { name }) => cmd_cd(&name),
        Some(Commands::Repo { action }) => match action {
            RepoAction::Add { path } => cmd_repo_add(path),
            RepoAction::Remove { name } => cmd_repo_remove(&name),
            RepoAction::List => cmd_repo_list(),
        },
        Some(Commands::Preset { action }) => match action {
            PresetAction::Add { name, repo } => cmd_preset_add(&name, &repo),
            PresetAction::Edit { name } => cmd_preset_edit(&name),
            PresetAction::Remove { name } => cmd_preset_remove(&name),
            PresetAction::List => cmd_preset_list(),
        },
        Some(Commands::Upgrade) => cmd_upgrade(),
        Some(Commands::Config { shell }) => match shell {
            ConfigShell::Zsh => cmd_config_zsh(),
            ConfigShell::Bash => cmd_config_bash(),
        },
        None => cmd_default(verbose),
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
    if let Ok(rename_file) = std::env::var("BOX_RENAME_FILE") {
        let _ = fs::write(rename_file, name);
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
fn cmd_default(verbose: bool) -> Result<i32> {
    if std::env::var_os("BOX_SESSION").is_some() {
        bail!(
            "Cannot nest box sessions (already inside session {:?}).",
            std::env::var("BOX_SESSION").unwrap_or_default()
        );
    }
    cmd_create_tui(verbose)
}

/// `box create` with no name: prompt for session details.
fn cmd_create_tui(verbose: bool) -> Result<i32> {
    let strategy = workspace::Strategy::resolve(None)?;
    match tui::create_session()? {
        tui::TuiAction::New { name, repos } => {
            if let Err(e) = update_repos(&repos, verbose) {
                eprintln!("Warning: repo update failed: {}", e);
            }
            cmd_create(&name, repos, strategy, verbose)
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

fn cmd_new(args: CreateArgs, verbose: bool) -> Result<i32> {
    if std::env::var_os("BOX_SESSION").is_some() {
        bail!(
            "Cannot nest box sessions (already inside session {:?}).",
            std::env::var("BOX_SESSION").unwrap_or_default()
        );
    }
    let repo_names = if let Some(preset_name) = &args.preset {
        preset::resolve(preset_name)?
    } else if !args.repo.is_empty() {
        args.repo
    } else {
        bail!("Either --repo or --preset is required.");
    };
    if let Err(e) = update_repos(&repo_names, verbose) {
        eprintln!("Warning: repo update failed: {}", e);
    }
    let strategy = workspace::Strategy::from_str(&args.strategy)?;
    cmd_create(&args.name, repo_names, strategy, verbose)
}

fn cmd_create(
    name: &str,
    repo_names: Vec<String>,
    strategy: workspace::Strategy,
    verbose: bool,
) -> Result<i32> {
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

    if verbose {
        eprintln!("\x1b[2msession:\x1b[0m {}", name);
        eprintln!("\x1b[2mrepos:\x1b[0m {}", cfg.repos.join(", "));
        eprintln!("\x1b[2mstrategy:\x1b[0m {}", strategy);
        eprintln!();
    } else {
        eprintln!(
            "\x1b[2msession:\x1b[0m {} \x1b[2m({} repos, {})\x1b[0m",
            name,
            cfg.repos.len(),
            strategy
        );
    }

    let mut sess = session::Session::from(cfg);
    sess.strategy = strategy.as_str().to_string();
    session::save(&sess)?;

    let workspace_path = workspace::ensure_workspace(name, &selected_repos, strategy, verbose)?;
    if selected_repos.len() == 1 {
        let repo_path = Path::new(&workspace_path).join(&selected_repos[0].name);
        output_cd_path(&repo_path.to_string_lossy());
    } else {
        output_cd_path(&workspace_path);
    }
    rename_terminal_tab(name);

    Ok(0)
}

fn cmd_edit(name: &str, verbose: bool) -> Result<i32> {
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
                workspace::ensure_workspace(name, &repos_to_add, strategy, verbose)?;
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

fn cmd_remove(name: &str, verbose: bool) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let sess = session::load(name)?;
    let strategy =
        workspace::Strategy::from_str(&sess.strategy).unwrap_or(workspace::Strategy::Clone);

    workspace::remove_workspace_by_strategy(name, &sess.repos, strategy, verbose);
    session::remove_dir(name)?;

    if !sess.project_dir.is_empty() {
        output_cd_path(&sess.project_dir);
    }
    println!("Session '{}' removed.", name);
    Ok(0)
}

fn cmd_remove_tui(verbose: bool) -> Result<i32> {
    match tui::select_sessions()? {
        tui::TuiAction::Remove { sessions } => {
            for name in &sessions {
                if let Ok(sess) = session::load(name) {
                    let strategy = workspace::Strategy::from_str(&sess.strategy)
                        .unwrap_or(workspace::Strategy::Clone);
                    workspace::remove_workspace_by_strategy(name, &sess.repos, strategy, verbose);
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

fn cmd_preset_add(name: &str, repos: &[String]) -> Result<i32> {
    if repos.is_empty() {
        // Interactive repo selection — pre-select existing preset repos if updating.
        // validate_name is called by load(), so path traversal is rejected.
        let current = match preset::load(name) {
            Ok(repos) => repos,
            Err(e) if e.to_string().contains("No preset named") => Vec::new(),
            Err(e) => return Err(e),
        };
        match tui::select_preset_repos(&current)? {
            tui::TuiAction::Edit { repos } => {
                preset::add(name, &repos)?;
            }
            _ => {
                return Ok(0);
            }
        }
    } else {
        preset::add(name, repos)?;
    }
    Ok(0)
}

fn cmd_preset_edit(name: &str) -> Result<i32> {
    let current = preset::load(name)?;
    match tui::select_preset_repos(&current)? {
        tui::TuiAction::Edit { repos } => {
            preset::add(name, &repos)?;
        }
        _ => {
            return Ok(0);
        }
    }
    Ok(0)
}

fn cmd_preset_remove(name: &str) -> Result<i32> {
    preset::remove(name)?;
    Ok(0)
}

fn cmd_preset_list() -> Result<i32> {
    let presets = preset::list()?;
    if presets.is_empty() {
        println!("No presets defined.");
        return Ok(0);
    }
    let name_w = presets
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(0)
        .max(4);

    println!("\x1b[2m  {:<name_w$}  REPOS\x1b[0m", "NAME");
    for (name, repos) in &presets {
        println!("  {:<name_w$}  {}", name, repos.join(", "));
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

__box_presets() {{
    local -a presets
    local __box_root="${{BOX_ROOT:-$HOME/.box}}"
    if [[ -d "$__box_root/presets" ]]; then
        for preset in "$__box_root/presets"/*(N.); do
            local name=${{preset:t}}
            [[ -n "$name" ]] && presets+=("$name")
        done
    fi
    if (( ${{#presets}} )); then
        _describe 'preset' presets
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
                'preset:Manage session presets'
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
                        '--preset=[Use a preset]:preset:__box_presets' \
                        '--strategy=[Workspace strategy]:strategy:(clone worktree)' \
                        '(-v --verbose)'{{-v,--verbose}}'[Show detailed output]' \
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
                    _arguments \
                        '(-v --verbose)'{{-v,--verbose}}'[Show detailed output]' \
                        '1:session name:__box_sessions'
                    ;;
                edit)
                    _arguments \
                        '(-v --verbose)'{{-v,--verbose}}'[Show detailed output]' \
                        '1:session name:__box_sessions'
                    ;;
                switch|sw|cd)
                    if (( CURRENT == 2 )); then
                        __box_sessions
                    fi
                    ;;
                repo)
                    if (( CURRENT == 2 )); then
                        local -a repo_subcmds
                        repo_subcmds=('add:Register a git repo' 'remove:Unregister a repo' 'rm:Unregister a repo' 'list:List registered repos' 'ls:List registered repos')
                        _describe 'repo subcommand' repo_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            remove|rm)
                                __box_repos
                                ;;
                            add)
                                _files -/
                                ;;
                        esac
                    fi
                    ;;
                preset)
                    if (( CURRENT == 2 )); then
                        local -a preset_subcmds
                        preset_subcmds=('add:Create or update a preset' 'edit:Edit repos in an existing preset' 'remove:Remove a preset' 'rm:Remove a preset' 'list:List presets' 'ls:List presets')
                        _describe 'preset subcommand' preset_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            edit|remove|rm)
                                __box_presets
                                ;;
                        esac
                    elif [[ $words[2] == "add" ]]; then
                        _arguments \
                            '*--repo=[Select specific repo]:repo:__box_repos'
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
    local __box_cd_file __box_rename_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    __box_rename_file=$(mktemp "/tmp/.box-rename.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" BOX_RENAME_FILE="$__box_rename_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    if [[ -s "$__box_rename_file" ]]; then
        local __box_name
        __box_name=$(<"$__box_rename_file")
        if [[ -n "$ZELLIJ" ]]; then
            command zellij action rename-tab "$__box_name" 2>/dev/null
        elif [[ -n "$TMUX" ]]; then
            command tmux rename-window -t "$TMUX_PANE" "$__box_name" 2>/dev/null
        fi
    fi
    rm -f "$__box_cd_file" "$__box_rename_file"
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

    local subcommands="new edit remove rm list switch sw cd repo preset upgrade config"
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
                    COMPREPLY=($(compgen -W "--repo --preset --strategy --verbose -v" -- "$cur"))
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
        edit|remove|rm)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--verbose -v" -- "$cur"))
                    ;;
                *)
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
            esac
            ;;
        switch|sw|cd)
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
                COMPREPLY=($(compgen -W "add remove rm list ls" -- "$cur"))
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
                        ;;
                esac
            fi
            ;;
        preset)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add edit remove rm list ls" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${{words[2]}}" in
                    edit|remove|rm)
                        local presets=""
                        if [[ -d "$__box_root/presets" ]]; then
                            for f in "$__box_root/presets"/*; do
                                [[ -f "$f" ]] || continue
                                presets+=" $(basename "$f")"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$presets" -- "$cur"))
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
    local __box_cd_file __box_rename_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    __box_rename_file=$(mktemp "/tmp/.box-rename.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" BOX_RENAME_FILE="$__box_rename_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    if [[ -s "$__box_rename_file" ]]; then
        local __box_name
        __box_name=$(<"$__box_rename_file")
        if [[ -n "$ZELLIJ" ]]; then
            command zellij action rename-tab "$__box_name" 2>/dev/null
        elif [[ -n "$TMUX" ]]; then
            command tmux rename-window -t "$TMUX_PANE" "$__box_name" 2>/dev/null
        fi
    fi
    rm -f "$__box_cd_file" "$__box_rename_file"
    return $__box_exit
}}
"#
    );
    Ok(0)
}

/// Fetch all refs for a bare repo, capturing output.
///
/// When worktrees have branches checked out, git refuses to update those refs
/// via fetch. We detect checked-out branches and exclude them with negative
/// refspecs. If git still refuses (e.g. a worktree admin entry we missed),
/// we parse the error, add the offending branch to the excludes, and retry.
///
/// Returns (success, captured_output) for use in parallel execution.
fn update_repo_captured(entry: &repo::RepoEntry) -> (bool, String) {
    let mut excludes = worktree_checked_out_branches(&entry.path);
    let mut log = String::new();

    for _ in 0..8 {
        let mut args: Vec<String> = vec![
            "-C".into(),
            entry.path.clone(),
            "fetch".into(),
            "origin".into(),
            "+refs/heads/*:refs/heads/*".into(),
        ];
        for branch in &excludes {
            args.push(format!("^refs/heads/{}", branch));
        }

        let result = std::process::Command::new("git")
            .args(args.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output();

        let output = match result {
            Ok(o) => o,
            Err(e) => {
                log.push_str(&format!("  \x1b[31mfetch error: {}\x1b[0m\n", e));
                return (false, log);
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if output.status.success() {
            log.push_str(&stdout);
            log.push_str(&stderr);
            return (true, log);
        }

        let new_excludes = parse_refused_branches(&stderr);
        let added: Vec<String> = new_excludes
            .into_iter()
            .filter(|b| !excludes.contains(b))
            .collect();

        if added.is_empty() {
            log.push_str(&stdout);
            log.push_str(&stderr);
            log.push_str("  \x1b[31mfetch failed\x1b[0m\n");
            return (false, log);
        }

        excludes.extend(added);
    }

    log.push_str("  \x1b[31mfetch failed: too many checked-out branches to exclude\x1b[0m\n");
    (false, log)
}

/// Return branch names currently checked out in any worktree of the given repo.
fn worktree_checked_out_branches(bare_path: &str) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["-C", bare_path, "worktree", "list", "--porcelain"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|l| l.strip_prefix("branch refs/heads/"))
        .map(String::from)
        .collect()
}

/// Extract branch names from git's "refusing to fetch into branch 'refs/heads/X'
/// checked out at..." error lines.
fn parse_refused_branches(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let Some(rest) = line
            .split_once("refusing to fetch into branch '")
            .map(|s| s.1)
        else {
            continue;
        };
        let Some((ref_name, _)) = rest.split_once('\'') else {
            continue;
        };
        if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
            out.push(branch.to_string());
        }
    }
    out
}

/// Fetch named repos in parallel.
fn update_repos(names: &[String], verbose: bool) -> Result<()> {
    let all_repos = repo::list()?;
    let items: Vec<(String, repo::RepoEntry)> = all_repos
        .into_iter()
        .filter(|r| names.contains(&r.name))
        .map(|r| (r.name.clone(), r))
        .collect();
    if items.is_empty() {
        return Ok(());
    }

    let count = items.len();
    if verbose {
        eprintln!("\x1b[2mUpdating repos…\x1b[0m");
    } else {
        eprint!(
            "\x1b[2mFetching {} repo{}…\x1b[0m ",
            count,
            if count == 1 { "" } else { "s" }
        );
    }
    let results = parallel::run_parallel(items, |_name, entry| update_repo_captured(&entry));

    if verbose {
        for result in &results {
            eprintln!("\x1b[1m{}\x1b[0m", result.name);
            if !result.output.is_empty() {
                eprint!("{}", result.output);
            }
            if result.success {
                eprintln!("  \x1b[32mok\x1b[0m");
            }
        }
    } else {
        let failures: Vec<&parallel::TaskResult> = results.iter().filter(|r| !r.success).collect();
        if failures.is_empty() {
            eprintln!("\x1b[32mok\x1b[0m");
        } else {
            eprintln!("\x1b[31m{} failed\x1b[0m", failures.len());
            for f in &failures {
                eprintln!("  \x1b[1m{}\x1b[0m: {}", f.name, f.output.trim());
            }
        }
    }

    Ok(())
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
    fn test_new_name_only_parses() {
        // --repo and --preset are both optional at parse time; runtime enforces at least one
        let cli = parse(&["new", "my-session"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert!(args.repo.is_empty());
                assert!(args.preset.is_none());
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_with_preset() {
        let cli = parse(&["new", "my-session", "--preset", "work"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.preset.as_deref(), Some("work"));
                assert!(args.repo.is_empty());
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_preset_and_repo_conflict() {
        let result = try_parse(&["new", "my-session", "--preset", "work", "--repo", "app"]);
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

    // -- preset subcommand --

    #[test]
    fn test_preset_add_parses() {
        let cli = parse(&[
            "preset", "add", "work", "--repo", "app-a", "--repo", "app-b",
        ]);
        match cli.command {
            Some(Commands::Preset {
                action: PresetAction::Add { name, repo },
            }) => {
                assert_eq!(name, "work");
                assert_eq!(repo, vec!["app-a", "app-b"]);
            }
            other => panic!("expected Preset Add, got {:?}", other),
        }
    }

    #[test]
    fn test_preset_edit_parses() {
        let cli = parse(&["preset", "edit", "work"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Preset {
                action: PresetAction::Edit { name }
            }) if name == "work"
        ));
    }

    #[test]
    fn test_preset_remove_parses() {
        let cli = parse(&["preset", "remove", "work"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Preset {
                action: PresetAction::Remove { name }
            }) if name == "work"
        ));
    }

    #[test]
    fn test_preset_remove_alias_rm() {
        let cli = parse(&["preset", "rm", "work"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Preset {
                action: PresetAction::Remove { name }
            }) if name == "work"
        ));
    }

    #[test]
    fn test_preset_list_parses() {
        let cli = parse(&["preset", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Preset {
                action: PresetAction::List
            })
        ));
    }

    #[test]
    fn test_preset_list_alias_ls() {
        let cli = parse(&["preset", "ls"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Preset {
                action: PresetAction::List
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

    #[test]
    fn test_parse_refused_branches_single() {
        let stderr = "fatal: refusing to fetch into branch 'refs/heads/box/conformance-2' checked out at '/Users/yusuke/.box/workspaces/conformance-2/jerboa'\n";
        assert_eq!(parse_refused_branches(stderr), vec!["box/conformance-2"]);
    }

    #[test]
    fn test_parse_refused_branches_multiple() {
        let stderr = "fatal: refusing to fetch into branch 'refs/heads/a' checked out at '/x'\n\
                      fatal: refusing to fetch into branch 'refs/heads/feat/b' checked out at '/y'\n";
        assert_eq!(parse_refused_branches(stderr), vec!["a", "feat/b"]);
    }

    #[test]
    fn test_parse_refused_branches_ignores_unrelated() {
        let stderr = "From github.com:org/repo\n   abc..def  main -> main\n";
        assert!(parse_refused_branches(stderr).is_empty());
    }
}
