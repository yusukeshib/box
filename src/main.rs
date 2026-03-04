mod config;
mod git;
mod repo;
mod session;
mod tui;
mod workspace;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "box",
    about = "Sandboxed git workspaces for development",
    after_help = "Examples:\n  box                                         # interactive session manager\n  box new my-feature                           # create a new session\n  box new my-feature -- bash                   # create with a command\n  box new my-feature --repo app-a --repo app-b # select specific repos\n  box exec my-feature -- ls -la                # run a command in a session\n  box list                                     # list all sessions\n  box remove my-feature                        # remove a session\n  box cd my-feature                            # print project directory\n  box path my-feature                          # print workspace path\n  box origin                                   # cd back to origin project from workspace\n  box repo add .                               # register current dir as a repo\n  box repo list                                # list registered repos\n  box repo remove my-app                       # unregister a repo\n  box upgrade                                  # self-update"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new session
    New(CreateArgs),
    /// Remove a session
    Remove(RemoveArgs),
    /// Run a command in a session
    Exec(ExecArgs),
    /// List sessions
    #[command(alias = "ls")]
    List(ListArgs),
    /// Print the host project directory for a session
    Cd {
        /// Session name
        name: String,
    },
    /// Print workspace path for a session
    Path {
        /// Session name
        name: String,
    },
    /// Navigate back to the original project directory from a workspace
    Origin,
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
    Remove {
        /// Repo name
        name: String,
    },
    /// List registered repos
    List,
}

#[derive(clap::Args, Debug)]
struct CreateArgs {
    /// Session name (omit to open the interactive session manager)
    name: Option<String>,

    /// Select specific repos by name (can be repeated; defaults to all)
    #[arg(long)]
    repo: Vec<String>,

    /// Command to run in the workspace (default: $BOX_DEFAULT_CMD if set)
    #[arg(last = true)]
    cmd: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct RemoveArgs {
    /// Session name
    name: String,
}

#[derive(clap::Args, Debug)]
struct ExecArgs {
    /// Session name
    name: String,

    /// Command to run in the workspace
    #[arg(last = true, required = true)]
    cmd: Vec<String>,
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
            match args.name {
                None => cmd_create_tui(),
                Some(name) => {
                    let cmd = if args.cmd.is_empty() {
                        None
                    } else {
                        Some(args.cmd)
                    };
                    let repos = if args.repo.is_empty() {
                        None
                    } else {
                        Some(args.repo)
                    };
                    cmd_create(&name, cmd, repos)
                }
            }
        }
        Some(Commands::Remove(args)) => cmd_remove(&args.name),
        Some(Commands::Exec(args)) => cmd_exec(&args.name, &args.cmd),
        Some(Commands::List(args)) => cmd_list_sessions(&args),
        Some(Commands::Cd { name }) => cmd_cd(&name),
        Some(Commands::Path { name }) => cmd_path(&name),
        Some(Commands::Origin) => cmd_origin(),
        Some(Commands::Repo { action }) => match action {
            RepoAction::Add { path } => cmd_repo_add(path),
            RepoAction::Remove { name } => cmd_repo_remove(&name),
            RepoAction::List => cmd_repo_list(),
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

fn run_local_command(name: &str, cmd: &[String]) -> Result<i32> {
    let home = config::home_dir()?;
    let workspace = Path::new(&home).join(".box").join("workspaces").join(name);
    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(workspace)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

fn output_cd_path(path: &str) {
    if let Ok(cd_file) = std::env::var("BOX_CD_FILE") {
        let _ = fs::write(cd_file, path);
    } else {
        println!("{}", path);
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
    if let Ok(home) = config::home_dir() {
        let workspaces = std::path::PathBuf::from(&home)
            .join(".box")
            .join("workspaces");
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

/// `box` with no args: alias for `box new` (interactive TUI).
fn cmd_default() -> Result<i32> {
    cmd_create_tui()
}

/// `box create` with no name: prompt for session details.
fn cmd_create_tui() -> Result<i32> {
    match tui::create_session()? {
        tui::TuiAction::New {
            name,
            command,
            repos,
        } => cmd_create(&name, command, Some(repos)),
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
    let command_w = sessions
        .iter()
        .map(|s| s.command.len())
        .max()
        .unwrap_or(0)
        .max(3);

    println!(
        "\x1b[2m  {:<name_w$}  {:<project_w$}  {:<command_w$}  CREATED\x1b[0m",
        "NAME", "PROJECT", "CMD",
    );

    for s in &sessions {
        let project = project_display(s);
        println!(
            "  {:<name_w$}  {:<project_w$}  {:<command_w$}  {}",
            s.name, project, s.command, s.created_at,
        );
    }

    Ok(0)
}

fn cmd_create(
    name: &str,
    cmd: Option<Vec<String>>,
    repo_names: Option<Vec<String>>,
) -> Result<i32> {
    session::validate_name(name)?;

    if session::session_exists(name)? {
        bail!("Session '{}' already exists.", name);
    }

    // Resolve repos
    let all_repos = repo::list()?;
    let selected_repos: Vec<repo::RepoEntry> = if let Some(names) = repo_names {
        if names.is_empty() {
            // TUI returned empty selection meaning use all
            all_repos.clone()
        } else {
            let mut result = Vec::new();
            for n in &names {
                let entry = all_repos
                    .iter()
                    .find(|r| r.name == *n)
                    .ok_or_else(|| anyhow::anyhow!("Repo '{}' not found in registry.", n))?;
                result.push(entry.clone());
            }
            result
        }
    } else {
        // CLI with no --repo flags: use all registered repos
        all_repos.clone()
    };

    if selected_repos.is_empty() {
        bail!("No repos registered. Run `box repo add <path>` first.");
    }

    let repo_names_list: Vec<String> = selected_repos.iter().map(|r| r.name.clone()).collect();

    // Resolve config (project_dir is empty for multi-repo sessions)
    let cfg = config::resolve(config::BoxConfigInput {
        name: name.to_string(),
        project_dir: String::new(),
        command: cmd,
        env: vec![],
        repos: repo_names_list,
    })?;

    eprintln!("\x1b[2msession:\x1b[0m {}", name);
    eprintln!("\x1b[2mrepos:\x1b[0m {}", cfg.repos.join(", "));
    if !cfg.command.is_empty() {
        eprintln!("\x1b[2mcommand:\x1b[0m {}", shell_words::join(&cfg.command));
    }
    eprintln!();

    let sess = session::Session::from(cfg);
    session::save(&sess)?;

    let home = config::home_dir()?;
    let workspace_path = workspace::ensure_workspace_multi(&home, name, &selected_repos)?;
    output_cd_path(&workspace_path);

    if !sess.command.is_empty() {
        return run_local_command(name, &sess.command);
    }
    Ok(0)
}

fn cmd_remove(name: &str) -> Result<i32> {
    session::validate_name(name)?;

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let sess = session::load(name)?;

    workspace::remove_workspace(name);
    session::remove_dir(name)?;

    if !sess.project_dir.is_empty() {
        output_cd_path(&sess.project_dir);
    }
    println!("Session '{}' removed.", name);
    Ok(0)
}

fn cmd_exec(name: &str, cmd: &[String]) -> Result<i32> {
    session::validate_name(name)?;

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let home = config::home_dir()?;
    let workspace_path = Path::new(&home).join(".box").join("workspaces").join(name);
    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(workspace_path)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

fn cmd_cd(name: &str) -> Result<i32> {
    session::validate_name(name)?;
    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }
    let home = config::home_dir()?;
    let path = Path::new(&home).join(".box").join("workspaces").join(name);
    output_cd_path(&path.to_string_lossy());
    Ok(0)
}

fn cmd_path(name: &str) -> Result<i32> {
    session::validate_name(name)?;
    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }
    let home = config::home_dir()?;
    let path = Path::new(&home).join(".box").join("workspaces").join(name);
    println!("{}", path.display());
    Ok(0)
}

fn cmd_origin() -> Result<i32> {
    let cwd = std::env::current_dir()?;
    let home = config::home_dir()?;
    let workspaces = Path::new(&home).join(".box").join("workspaces");
    let workspaces = std::fs::canonicalize(&workspaces).unwrap_or(workspaces);
    let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());

    let ws_name = cwd_canon
        .strip_prefix(&workspaces)
        .ok()
        .and_then(|rel| rel.components().next())
        .map(|c| c.as_os_str().to_string_lossy().to_string());

    let ws_name = match ws_name {
        Some(n) => n,
        None => bail!("Not inside a box workspace."),
    };

    if !session::session_exists(&ws_name)? {
        bail!("Session '{}' not found.", ws_name);
    }

    let sess = session::load(&ws_name)?;

    // For multi-repo sessions, detect which repo subdir we're in and look up its path
    if !sess.repos.is_empty() {
        let ws_root = Path::new(&home)
            .join(".box")
            .join("workspaces")
            .join(&ws_name);
        let ws_root_canon = std::fs::canonicalize(&ws_root).unwrap_or(ws_root);
        if let Ok(rel) = cwd_canon.strip_prefix(&ws_root_canon) {
            if let Some(repo_dir_name) = rel.components().next() {
                let repo_name = repo_dir_name.as_os_str().to_string_lossy().to_string();
                if let Ok(repos) = repo::list() {
                    if let Some(entry) = repos.iter().find(|r| r.name == repo_name) {
                        output_cd_path(&entry.path);
                        return Ok(0);
                    }
                }
            }
        }
        bail!("Navigate into a repo subdirectory first (e.g. cd <repo-name>).");
    }

    if sess.project_dir.is_empty() {
        bail!("Session '{}' has no origin project directory.", ws_name);
    }
    output_cd_path(&sess.project_dir);
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
    for r in &repos {
        println!("  {}  {}", r.name, r.path);
    }
    Ok(0)
}

fn cmd_config_zsh() -> Result<i32> {
    print!(
        r#"__box_sessions() {{
    local -a sessions
    if [[ -d "$HOME/.box/sessions" ]]; then
        for sess in "$HOME/.box/sessions"/*(N/); do
            if [[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]; then
                local sess_name=${{sess:t}}
                local desc=""
                if [[ -f "$sess/project_dir" ]]; then
                    desc=$(< "$sess/project_dir")
                    desc=${{desc/#$HOME/\~}}
                fi
                sessions+=("$sess_name:[$desc]")
            fi
        done
    fi
    if (( ${{#sessions}} )); then
        _describe 'session' sessions
    fi
}}

__box_repos() {{
    local -a repos
    if [[ -f "$HOME/.box/repos" ]]; then
        while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            repos+=("${{line##*/}}")
        done < "$HOME/.box/repos"
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
                'remove:Remove a session'
                'exec:Run a command in a session'
                'list:List sessions'
                'cd:Print the host project directory for a session'
                'path:Print workspace path for a session'
                'origin:Navigate back to the original project directory'
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
                        '1:session name:' \
                        '*:command:'
                    ;;
                exec)
                    _arguments \
                        '1:session name:__box_sessions' \
                        '*:command:'
                    ;;
                list|ls)
                    _arguments \
                        '--project[Show only sessions for the current project]' \
                        '-p[Show only sessions for the current project]' \
                        '--quiet[Only print session names]' \
                        '-q[Only print session names]'
                    ;;
                remove|path|cd)
                    if (( CURRENT == 2 )); then
                        __box_sessions
                    fi
                    ;;
                repo)
                    if (( CURRENT == 2 )); then
                        local -a repo_subcmds
                        repo_subcmds=('add:Register a git repo' 'remove:Unregister a repo' 'list:List registered repos')
                        _describe 'repo subcommand' repo_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            remove)
                                __box_repos
                                ;;
                            add)
                                _files -/
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

    local subcommands="new remove exec list cd path origin repo upgrade config"
    local session_cmds="remove exec cd path"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        return
    fi

    local subcmd="${{words[1]}}"

    case "$subcmd" in
        new)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--repo" -- "$cur"))
                    ;;
            esac
            ;;
        exec)
            if [[ $cword -eq 2 ]]; then
                local sessions=""
                if [[ -d "$HOME/.box/sessions" ]]; then
                    for sess in "$HOME/.box/sessions"/*/; do
                        ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                    done
                fi
                COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
            fi
            ;;
        list|ls)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--project -p --quiet -q" -- "$cur"))
                    ;;
            esac
            ;;
        remove|path|cd)
            if [[ $cword -eq 2 ]]; then
                local sessions=""
                if [[ -d "$HOME/.box/sessions" ]]; then
                    for sess in "$HOME/.box/sessions"/*/; do
                        ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                    done
                fi
                COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
            fi
            ;;
        repo)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add remove list" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${{words[2]}}" in
                    remove)
                        local repos=""
                        if [[ -f "$HOME/.box/repos" ]]; then
                            while IFS= read -r line; do
                                [[ -z "$line" ]] && continue
                                repos+=" ${{line##*/}}"
                            done < "$HOME/.box/repos"
                        fi
                        COMPREPLY=($(compgen -W "$repos" -- "$cur"))
                        ;;
                    add)
                        COMPREPLY=($(compgen -d -- "$cur"))
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
        let cli = parse(&["new", "my-session"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.name.as_deref(), Some("my-session"));
                assert!(args.cmd.is_empty());
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_with_command() {
        let cli = parse(&["new", "my-session", "--", "bash", "-c", "echo hi"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.name.as_deref(), Some("my-session"));
                assert_eq!(args.cmd, vec!["bash", "-c", "echo hi"]);
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn test_new_no_name_opens_tui() {
        let cli = parse(&["new"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert!(args.name.is_none());
            }
            _ => panic!("expected New"),
        }
    }

    #[test]
    fn test_new_with_repo_flags() {
        let cli = parse(&["new", "my-session", "--repo", "app-a", "--repo", "app-b"]);
        match cli.command {
            Some(Commands::New(args)) => {
                assert_eq!(args.name.as_deref(), Some("my-session"));
                assert_eq!(args.repo, vec!["app-a", "app-b"]);
            }
            other => panic!("expected New, got {:?}", other),
        }
    }

    // -- remove subcommand --

    #[test]
    fn test_remove_parses() {
        let cli = parse(&["remove", "my-session"]);
        match cli.command {
            Some(Commands::Remove(args)) => {
                assert_eq!(args.name, "my-session");
            }
            other => panic!("expected Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_requires_name() {
        let result = try_parse(&["remove"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_rejects_unknown_flags() {
        let result = try_parse(&["remove", "my-session", "-d"]);
        assert!(result.is_err());
    }

    // -- exec subcommand --

    #[test]
    fn test_exec_parses() {
        let cli = parse(&["exec", "my-session", "--", "ls", "-la"]);
        match cli.command {
            Some(Commands::Exec(args)) => {
                assert_eq!(args.name, "my-session");
                assert_eq!(args.cmd, vec!["ls", "-la"]);
            }
            other => panic!("expected Exec, got {:?}", other),
        }
    }

    #[test]
    fn test_exec_requires_name() {
        let result = try_parse(&["exec"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_exec_requires_command() {
        let result = try_parse(&["exec", "my-session"]);
        assert!(result.is_err());
    }

    // -- path subcommand --

    #[test]
    fn test_path_subcommand_parses() {
        let cli = parse(&["path", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Path { ref name }) if name == "my-session"
        ));
    }

    #[test]
    fn test_path_requires_name() {
        let result = try_parse(&["path"]);
        assert!(result.is_err());
    }

    // -- cd subcommand --

    #[test]
    fn test_cd_subcommand_parses() {
        let cli = parse(&["cd", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Cd { ref name }) if name == "my-session"
        ));
    }

    #[test]
    fn test_cd_requires_name() {
        let result = try_parse(&["cd"]);
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
    fn test_repo_list() {
        let cli = parse(&["repo", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Repo {
                action: RepoAction::List
            })
        ));
    }

    // -- bare name is rejected (subcommand required) --

    #[test]
    fn test_bare_name_rejected() {
        let result = try_parse(&["my-session"]);
        assert!(result.is_err());
    }
}
