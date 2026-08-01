mod config;
mod git;
mod parallel;
mod preset;
mod progress;
mod repo;
mod session;
#[cfg(test)]
mod test_util;
mod workspace;

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser, Subcommand};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "box",
    about = "Sandboxed git workspaces for development",
    long_about = "box manages sandboxed git workspaces for development.\n\n\
MENTAL MODEL (three tiers):\n\
  source     an upstream git repo registered via `box source add`; bare-cloned to ~/.box/repos/.\n\
  workspace  a named sandbox under ~/.box/workspaces/<name>/ built from one or more sources.\n\
  repo       a source checked out inside a workspace (also the unit listed in a preset).\n\
  preset     a named, reusable set of sources used to create workspaces.\n\n\
TYPICAL FLOW:\n\
  1. box source add <url|path>                  register a source\n\
  2. box workspace add <name> --repo <source>   create a workspace (alias: ws)\n\
  3. box workspace switch <name>                enter it (alias: sw)\n\
  4. box repo add <source> --workspace <name>   add/remove repos later\n\n\
CONVENTIONS (important for scripting/agents):\n\
  - Every command targets an EXPLICIT object. There is no implicit current-directory resolution.\n\
  - `box repo add|remove|list` requires exactly one of --workspace <name> or --preset <name>.\n\
  - `box rebase` requires --workspace <name> and --repo <name>.\n\
  - Subcommand aliases: workspace=ws, and within each group list=ls, remove=rm, switch=sw.\n\
  - `box workspace remove` prunes workspaces older than one day by default.",
    after_help = "Examples:\n  box workspace add my-feature --repo app-a    # create a workspace (alias: ws)\n  box workspace add my-feature --preset work   # create from a preset\n  box workspace list                           # list workspaces (alias: ls)\n  box workspace switch my-feature              # switch into a workspace (alias: sw)\n  box workspace remove my-feature              # remove a workspace (alias: rm)\n  box repo add app-c --workspace my-feature    # add a repo to a workspace\n  box repo remove app-a --workspace my-feature # remove a repo from a workspace\n  box repo list --workspace my-feature         # list repos in a workspace\n  box repo add app-c --preset work             # add a repo to a preset\n  box source add git@github.com:user/app.git   # register a source from a URL\n  box source add .                             # register current dir as a source\n  box source list                              # list registered sources\n  box source remove my-app                     # unregister a source\n  box preset add work --repo app-a --repo app-b # define a new preset\n  box preset update work --repo app-a --repo app-c # replace a preset's repos\n  box preset list                              # list presets\n  box rebase main --workspace my-feature --repo app-a # fetch & rebase a repo\n  box upgrade                                  # self-update"
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
    /// Manage workspaces (create, list, switch, remove)
    #[command(alias = "ws")]
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Manage repos within a workspace or preset
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Manage registered sources (upstream git repos)
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
    /// Manage presets
    Preset {
        #[command(subcommand)]
        action: PresetAction,
    },
    /// Fetch origin and rebase a workspace repo onto another branch
    Rebase(RebaseArgs),
    /// Self-update to the latest version
    Upgrade,
    /// Output shell configuration (e.g. eval "$(box config zsh)")
    Config {
        #[command(subcommand)]
        shell: ConfigShell,
    },
}

#[derive(Subcommand, Debug)]
enum WorkspaceAction {
    /// Create a new workspace
    Add(CreateArgs),
    /// List workspaces
    #[command(alias = "ls")]
    List(ListArgs),
    /// Remove one workspace, or prune old workspaces when NAME is omitted
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    /// Switch into a workspace
    #[command(alias = "sw")]
    Switch {
        /// Workspace name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum RepoAction {
    /// Add repo(s) to a workspace or preset
    Add(RepoEditArgs),
    /// Remove repo(s) from a workspace or preset
    #[command(alias = "rm")]
    Remove(RepoEditArgs),
    /// List repos in a workspace or preset
    #[command(alias = "ls")]
    List(RepoListArgs),
}

#[derive(Subcommand, Debug)]
enum SourceAction {
    /// Register a git repo as a source
    Add {
        /// Git remote URL (e.g. git@github.com:user/app.git) or local path
        /// to a repo (use `.` for the current directory)
        src: String,
    },
    /// Unregister a source by name
    #[command(alias = "rm")]
    Remove {
        /// Source name
        name: String,
    },
    /// List registered sources
    #[command(alias = "ls")]
    List,
}

#[derive(Subcommand, Debug)]
enum PresetAction {
    /// Create a new preset (fails if it already exists)
    Add {
        /// Preset name
        name: String,
        /// Repos to include (can be repeated)
        #[arg(long, required = true)]
        repo: Vec<String>,
    },
    /// Replace an existing preset's repos (fails if it doesn't exist)
    Update {
        /// Preset name
        name: String,
        /// Repos to include (can be repeated)
        #[arg(long, required = true)]
        repo: Vec<String>,
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
    /// Workspace name
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

    /// Skip the per-repo `git fetch origin` before creating the workspace
    #[arg(long)]
    no_fetch: bool,
}

#[derive(clap::Args, Debug)]
struct RepoEditArgs {
    /// Repo name(s)
    #[arg(required = true)]
    repo: Vec<String>,

    /// Target workspace (mutually exclusive with --preset)
    #[arg(long, group = "repo_target")]
    workspace: Option<String>,

    /// Target preset (mutually exclusive with --workspace)
    #[arg(long, group = "repo_target")]
    preset: Option<String>,
}

#[derive(clap::Args, Debug)]
struct RepoListArgs {
    /// Target workspace (mutually exclusive with --preset)
    #[arg(long, group = "repo_target")]
    workspace: Option<String>,

    /// Target preset (mutually exclusive with --workspace)
    #[arg(long, group = "repo_target")]
    preset: Option<String>,
}

#[derive(clap::Args, Debug)]
struct RemoveArgs {
    /// Workspace name (omit to prune old workspaces)
    name: Option<String>,

    /// Remove every workspace
    #[arg(long, short = 'a', conflicts_with = "name")]
    all: bool,

    /// Prune workspaces at least this old (supports s, m, h, d; default: 1d)
    #[arg(long, conflicts_with_all = ["name", "all"])]
    older_than: Option<String>,
}

#[derive(clap::Args, Debug)]
struct ListArgs {
    /// Only print workspace names
    #[arg(long, short)]
    quiet: bool,
}

#[derive(clap::Args, Debug)]
struct RebaseArgs {
    /// Branch to rebase onto (e.g. main)
    branch: String,
    /// Workspace name
    #[arg(long)]
    workspace: String,
    /// Repo name within the workspace
    #[arg(long)]
    repo: String,
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
        Some(Commands::Workspace { action }) => match action {
            WorkspaceAction::Add(args) => cmd_new(args, verbose),
            WorkspaceAction::List(args) => cmd_list_sessions(&args),
            WorkspaceAction::Remove(args) => {
                if args.all {
                    cmd_remove_all(verbose)
                } else if let Some(name) = &args.name {
                    cmd_remove(name, verbose)
                } else {
                    cmd_prune(args.older_than.as_deref().unwrap_or("1d"), verbose)
                }
            }
            WorkspaceAction::Switch { name } => cmd_cd(&name),
        },
        Some(Commands::Repo { action }) => match action {
            RepoAction::Add(args) => {
                cmd_repo_modify(&args.repo, args.workspace, args.preset, true, verbose)
            }
            RepoAction::Remove(args) => {
                cmd_repo_modify(&args.repo, args.workspace, args.preset, false, verbose)
            }
            RepoAction::List(args) => cmd_repo_list_target(args.workspace, args.preset),
        },
        Some(Commands::Source { action }) => match action {
            SourceAction::Add { src } => cmd_source_add(&src),
            SourceAction::Remove { name } => cmd_source_remove(&name),
            SourceAction::List => cmd_source_list(),
        },
        Some(Commands::Preset { action }) => match action {
            PresetAction::Add { name, repo } => cmd_preset_add(&name, &repo),
            PresetAction::Update { name, repo } => cmd_preset_update(&name, &repo),
            PresetAction::Remove { name } => cmd_preset_remove(&name),
            PresetAction::List => cmd_preset_list(),
        },
        Some(Commands::Rebase(args)) => {
            cmd_rebase(&args.branch, &args.workspace, &args.repo, verbose)
        }
        Some(Commands::Upgrade) => cmd_upgrade(),
        Some(Commands::Config { shell }) => match shell {
            ConfigShell::Zsh => cmd_config_zsh(),
            ConfigShell::Bash => cmd_config_bash(),
        },
        None => cmd_help(),
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_help() -> Result<i32> {
    Cli::command().print_help()?;
    println!();
    Ok(0)
}

fn output_cd_path(path: &str) {
    if let Ok(cd_file) = std::env::var("BOX_CD_FILE") {
        let _ = fs::write(cd_file, path);
    } else {
        println!("{}", path);
    }
}

fn signal_post_switch_hook(name: &str) {
    if let Ok(hook_file) = std::env::var("BOX_POST_SWITCH_FILE") {
        let _ = fs::write(hook_file, name);
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

fn cmd_list_sessions(args: &ListArgs) -> Result<i32> {
    let sessions = session::list()?;

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
    let strategy = workspace::Strategy::from_str(&args.strategy)?;
    cmd_create(&args.name, repo_names, strategy, !args.no_fetch, verbose)
}

fn cmd_create(
    name: &str,
    repo_names: Vec<String>,
    strategy: workspace::Strategy,
    fetch: bool,
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
        bail!("No sources registered. Run `box source add <url>` first.");
    }

    let repo_names_list: Vec<String> = selected_repos.iter().map(|r| r.name.clone()).collect();

    if verbose {
        eprintln!("\x1b[2msession:\x1b[0m {}", name);
        eprintln!("\x1b[2mrepos:\x1b[0m {}", repo_names_list.join(", "));
        eprintln!("\x1b[2mstrategy:\x1b[0m {}", strategy);
        eprintln!();
    } else {
        eprintln!(
            "\x1b[2msession:\x1b[0m {} \x1b[2m({} repos, {})\x1b[0m",
            name,
            repo_names_list.len(),
            strategy
        );
    }

    // project_dir is empty for sessions — repos are the source of truth.
    let sess = session::Session {
        name: name.to_string(),
        project_dir: String::new(),
        repos: repo_names_list,
        strategy: strategy.as_str().to_string(),
    };
    session::save(&sess)?;

    let workspace_path =
        workspace::ensure_workspace(name, &selected_repos, strategy, fetch, verbose)?;
    if selected_repos.len() == 1 {
        let repo_path = Path::new(&workspace_path).join(&selected_repos[0].name);
        output_cd_path(&repo_path.to_string_lossy());
    } else {
        output_cd_path(&workspace_path);
    }
    signal_post_switch_hook(name);

    Ok(0)
}

/// Target of a `box repo` operation: either a workspace or a preset.
enum RepoTarget {
    Workspace(String),
    Preset(String),
}

/// Resolve the mutually-exclusive --workspace / --preset target, requiring
/// exactly one to be set. (clap's `repo_target` group already prevents both.)
fn resolve_repo_target(workspace: Option<String>, preset: Option<String>) -> Result<RepoTarget> {
    match (workspace, preset) {
        (Some(w), None) => Ok(RepoTarget::Workspace(w)),
        (None, Some(p)) => Ok(RepoTarget::Preset(p)),
        (None, None) => bail!("Specify a target with --workspace <name> or --preset <name>."),
        (Some(_), Some(_)) => unreachable!("--workspace and --preset are mutually exclusive"),
    }
}

/// Add or remove repos in a workspace or preset (replaces the old `box edit`).
fn cmd_repo_modify(
    repos: &[String],
    workspace: Option<String>,
    preset: Option<String>,
    adding: bool,
    verbose: bool,
) -> Result<i32> {
    let (add, remove): (&[String], &[String]) = if adding { (repos, &[]) } else { (&[], repos) };

    match resolve_repo_target(workspace, preset)? {
        RepoTarget::Workspace(name) => {
            let name = session::validate_name(&name)?;
            let name = name.as_str();
            if !session::session_exists(name)? {
                bail!("Workspace '{}' not found.", name);
            }
            let sess = session::load(name)?;
            let strategy = workspace::Strategy::from_str(&sess.strategy)?;
            let new_repos = compute_edit_repos(&sess.repos, add, remove)?;
            ensure_workspace_has_repos(name, &new_repos)?;
            apply_edit_diff(name, &sess.repos, &new_repos, strategy, verbose)
        }
        RepoTarget::Preset(name) => {
            let current = preset::load(&name)?;
            let new_repos = compute_edit_repos(&current, add, remove)?;

            let added: Vec<&str> = new_repos
                .iter()
                .filter(|r| !current.contains(r))
                .map(|r| r.as_str())
                .collect();
            let removed: Vec<&str> = current
                .iter()
                .filter(|r| !new_repos.contains(r))
                .map(|r| r.as_str())
                .collect();

            preset::update(&name, &new_repos)?;

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
    }
}

/// List the repos in a workspace or preset.
fn cmd_repo_list_target(workspace: Option<String>, preset: Option<String>) -> Result<i32> {
    let repos = match resolve_repo_target(workspace, preset)? {
        RepoTarget::Workspace(name) => {
            let name = session::validate_name(&name)?;
            let name = name.as_str();
            if !session::session_exists(name)? {
                bail!("Workspace '{}' not found.", name);
            }
            session::load(name)?.repos
        }
        RepoTarget::Preset(name) => preset::load(&name)?,
    };

    if repos.is_empty() {
        println!("No repos.");
        return Ok(0);
    }
    for r in &repos {
        println!("{}", r);
    }
    Ok(0)
}

fn ensure_workspace_has_repos(name: &str, repos: &[String]) -> Result<()> {
    if repos.is_empty() {
        bail!(
            "Cannot remove the final repo from workspace '{}'. Remove the workspace instead.",
            name
        );
    }
    Ok(())
}

/// Compute the new repo set from --add/--remove flags. Errors when the same
/// repo appears in both lists or when an --add target is not registered.
/// Already-present adds and not-present removes warn but do not error.
fn compute_edit_repos(
    current: &[String],
    add: &[String],
    remove: &[String],
) -> Result<Vec<String>> {
    if let Some(dup) = add.iter().find(|r| remove.contains(r)) {
        bail!("'{}' appears in both --add and --remove", dup);
    }

    let all_repos = repo::list()?;
    for repo_name in add {
        if !all_repos.iter().any(|r| &r.name == repo_name) {
            bail!(
                "Repo '{}' is not registered. Run `box source add` first.",
                repo_name
            );
        }
    }

    for repo_name in add {
        if current.contains(repo_name) {
            eprintln!(
                "\x1b[2mskip:\x1b[0m '{}' is already in the session",
                repo_name
            );
        }
    }
    for repo_name in remove {
        if !current.contains(repo_name) {
            eprintln!("\x1b[2mskip:\x1b[0m '{}' is not in the session", repo_name);
        }
    }

    let mut new_repos: Vec<String> = current
        .iter()
        .filter(|r| !remove.contains(r))
        .cloned()
        .collect();
    for repo_name in add {
        if !new_repos.contains(repo_name) {
            new_repos.push(repo_name.clone());
        }
    }
    Ok(new_repos)
}

fn apply_edit_diff(
    name: &str,
    current_repos: &[String],
    new_repos: &[String],
    strategy: workspace::Strategy,
    verbose: bool,
) -> Result<i32> {
    let all_repos = repo::list()?;

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

    if !added.is_empty() {
        let repos_to_add: Vec<repo::RepoEntry> = added
            .iter()
            .filter_map(|n| all_repos.iter().find(|r| r.name == *n).cloned())
            .collect();
        workspace::ensure_workspace(name, &repos_to_add, strategy, false, verbose)?;
    }

    for repo_name in &removed {
        workspace::remove_repo_by_strategy(name, repo_name, strategy);
    }

    session::update_repos(name, new_repos)?;

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

fn cmd_remove(name: &str, verbose: bool) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();

    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }

    let sess = session::load(name)?;
    let strategy =
        workspace::Strategy::from_str(&sess.strategy).unwrap_or(workspace::Strategy::Clone);

    let failed =
        workspace::remove_sessions(&[(name.to_string(), strategy, sess.repos.clone())], verbose)?;
    if failed.contains(name) {
        bail!(
            "Failed to remove workspace '{}'; session metadata was retained.",
            name
        );
    }
    session::remove_dir(name)?;

    if !sess.project_dir.is_empty() {
        output_cd_path(&sess.project_dir);
    }
    println!("Session '{}' removed.", name);
    Ok(0)
}

fn parse_age(value: &str) -> Result<chrono::Duration> {
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let amount: i64 = number.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid age '{}'. Use a positive value such as 12h or 7d.",
            value
        )
    })?;
    if amount <= 0 {
        bail!("Age must be greater than zero.");
    }
    let duration = match unit {
        "s" => chrono::Duration::try_seconds(amount),
        "m" => chrono::Duration::try_minutes(amount),
        "h" => chrono::Duration::try_hours(amount),
        "d" => chrono::Duration::try_days(amount),
        _ => bail!("Invalid age '{}'. Supported units: s, m, h, d.", value),
    };
    duration.ok_or_else(|| anyhow::anyhow!("Age '{}' is too large.", value))
}

fn active_workspace_names() -> std::collections::BTreeSet<String> {
    let mut active = std::collections::BTreeSet::new();
    if let Ok(name) = std::env::var("BOX_SESSION") {
        if !name.is_empty() {
            active.insert(name);
        }
    }
    if let (Ok(root), Ok(cwd)) = (config::box_root(), std::env::current_dir()) {
        let workspaces = root.join("workspaces");
        if let Ok(relative) = cwd.strip_prefix(workspaces) {
            if let Some(component) = relative.components().next() {
                active.insert(component.as_os_str().to_string_lossy().into_owned());
            }
        }
    }
    active
}

fn is_prunable(
    name: &str,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    cutoff: chrono::DateTime<chrono::Utc>,
    active: &std::collections::BTreeSet<String>,
) -> bool {
    !active.contains(name) && created_at.is_some_and(|created| created <= cutoff)
}

fn cmd_prune(older_than: &str, verbose: bool) -> Result<i32> {
    let age = parse_age(older_than)?;
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(age)
        .ok_or_else(|| anyhow::anyhow!("Age '{}' is too large.", older_than))?;
    let active = active_workspace_names();
    let mut to_remove = Vec::new();

    for summary in session::list()? {
        let created_at = session::created_at(&summary.name)?;
        if !is_prunable(&summary.name, created_at, cutoff, &active) {
            if verbose && active.contains(&summary.name) {
                eprintln!("Skipping active workspace '{}'.", summary.name);
            } else if verbose && created_at.is_none() {
                eprintln!(
                    "Skipping '{}' because created_at is missing or invalid.",
                    summary.name
                );
            }
            continue;
        }
        let sess = session::load(&summary.name)?;
        let strategy =
            workspace::Strategy::from_str(&sess.strategy).unwrap_or(workspace::Strategy::Clone);
        to_remove.push((summary.name, strategy, sess.repos));
    }

    if to_remove.is_empty() {
        println!("No workspaces older than {}.", older_than);
        return Ok(0);
    }

    let failed = workspace::remove_sessions(&to_remove, verbose)?;
    for (name, _, _) in &to_remove {
        if !failed.contains(name) {
            session::remove_dir(name)?;
            println!("Session '{}' removed.", name);
        }
    }
    if !failed.is_empty() {
        bail!(
            "Failed to remove workspace(s): {}. Session metadata was retained.",
            failed.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(0)
}

fn cmd_remove_all(verbose: bool) -> Result<i32> {
    let sessions = session::list()?;
    if sessions.is_empty() {
        eprintln!("No sessions to remove.");
        return Ok(0);
    }

    let to_remove: Vec<(String, workspace::Strategy, Vec<String>)> = sessions
        .iter()
        .map(|s| {
            let strategy =
                workspace::Strategy::from_str(&s.strategy).unwrap_or(workspace::Strategy::Clone);
            (s.name.clone(), strategy, s.repos.clone())
        })
        .collect();

    let failed = workspace::remove_sessions(&to_remove, verbose)?;
    for s in &sessions {
        if !failed.contains(&s.name) {
            session::remove_dir(&s.name)?;
            eprintln!("Session '{}' removed.", s.name);
        }
    }
    if !failed.is_empty() {
        bail!(
            "Failed to remove workspace(s): {}. Session metadata was retained.",
            failed.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(0)
}

fn cmd_cd(name: &str) -> Result<i32> {
    let name = session::validate_name(name)?;
    let name = name.as_str();
    if !session::session_exists(name)? {
        bail!("Session '{}' not found.", name);
    }
    let path = resolve_workspace_path(name)?;
    output_cd_path(&path.to_string_lossy());
    signal_post_switch_hook(name);
    Ok(0)
}

/// Fetch origin in the bare repo backing a workspace repo, then rebase that
/// repo's current branch onto `branch`.
///
/// Box workspaces share a single bare repo across worktrees, and `git fetch`
/// from a worktree often refuses to update sibling-worktree branches. This
/// command routes the fetch through `git::fetch_repo`, which knows how to
/// exclude checked-out branches with negative refspecs.
fn cmd_rebase(branch: &str, workspace: &str, repo: &str, verbose: bool) -> Result<i32> {
    let workspace = session::validate_name(workspace)?;
    if !session::session_exists(&workspace)? {
        bail!("Workspace '{}' not found.", workspace);
    }
    let worktree_root = config::box_root()?
        .join("workspaces")
        .join(&workspace)
        .join(repo);
    if !worktree_root.is_dir() {
        bail!("Repo '{}' not found in workspace '{}'.", repo, workspace);
    }
    let worktree_root_str = worktree_root.to_string_lossy().to_string();

    let output = std::process::Command::new("git")
        .args(["-C", &worktree_root_str, "rev-parse", "--git-common-dir"])
        .output()?;
    if !output.status.success() {
        bail!("Failed to determine git common directory.");
    }
    let common = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let common_path = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        worktree_root.join(common)
    };
    let common_canonical =
        std::fs::canonicalize(&common_path).unwrap_or_else(|_| common_path.clone());
    let common_str = common_canonical.to_string_lossy().to_string();

    // Resolve the matching registered repo so fetch_repo's log uses the
    // user-facing name; if cwd isn't a box-managed worktree we still attempt
    // the fetch via a synthetic entry pointing at the common dir.
    let entry = repo::list()
        .ok()
        .and_then(|all| {
            all.into_iter().find(|r| {
                std::fs::canonicalize(&r.path)
                    .map(|p| p.to_string_lossy() == common_str)
                    .unwrap_or(false)
            })
        })
        .unwrap_or_else(|| repo::RepoEntry {
            name: common_canonical
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "repo".to_string()),
            path: common_str.clone(),
        });

    eprintln!("\x1b[2mfetching origin in {}…\x1b[0m", entry.name);
    let (ok, log) = git::fetch_repo(&entry);
    if verbose || !ok {
        eprint!("{}", log);
    }
    if !ok {
        bail!("Fetch failed.");
    }

    let status = std::process::Command::new("git")
        .args(["-C", &worktree_root_str, "rebase", branch])
        .status()?;
    Ok(if status.success() { 0 } else { 1 })
}

fn cmd_source_add(src: &str) -> Result<i32> {
    repo::add(src)?;
    Ok(0)
}

fn cmd_source_remove(name: &str) -> Result<i32> {
    repo::remove(name)?;
    Ok(0)
}

fn cmd_source_list() -> Result<i32> {
    let repos = repo::list()?;
    if repos.is_empty() {
        println!("No sources registered.");
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
    preset::add(name, repos)?;
    Ok(0)
}

fn cmd_preset_update(name: &str, repos: &[String]) -> Result<i32> {
    preset::update(name, repos)?;
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
    print!("{}", include_str!("completions/box.zsh"));
    Ok(0)
}

fn cmd_config_bash() -> Result<i32> {
    print!("{}", include_str!("completions/box.bash"));
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

    // -- No args = help --

    #[test]
    fn test_no_args_selects_help() {
        let cli = parse(&[]);
        assert!(cli.command.is_none());
    }

    // -- workspace add subcommand --

    fn ws_add(cli: Cli) -> CreateArgs {
        match cli.command {
            Some(Commands::Workspace {
                action: WorkspaceAction::Add(args),
            }) => args,
            other => panic!("expected Workspace Add, got {:?}", other),
        }
    }

    #[test]
    fn test_workspace_add_name_only() {
        let args = ws_add(parse(&["workspace", "add", "my-session", "--repo", "app"]));
        assert_eq!(args.name, "my-session");
        assert_eq!(args.repo, vec!["app"]);
    }

    #[test]
    fn test_workspace_alias_ws() {
        let args = ws_add(parse(&["ws", "add", "my-session", "--repo", "app"]));
        assert_eq!(args.name, "my-session");
    }

    #[test]
    fn test_workspace_add_requires_name() {
        assert!(try_parse(&["workspace", "add"]).is_err());
    }

    #[test]
    fn test_workspace_add_name_only_parses() {
        // --repo and --preset are both optional at parse time; runtime enforces at least one
        let args = ws_add(parse(&["workspace", "add", "my-session"]));
        assert!(args.repo.is_empty());
        assert!(args.preset.is_none());
    }

    #[test]
    fn test_workspace_add_with_preset() {
        let args = ws_add(parse(&[
            "workspace",
            "add",
            "my-session",
            "--preset",
            "work",
        ]));
        assert_eq!(args.preset.as_deref(), Some("work"));
        assert!(args.repo.is_empty());
    }

    #[test]
    fn test_workspace_add_preset_and_repo_conflict() {
        assert!(try_parse(&[
            "workspace",
            "add",
            "my-session",
            "--preset",
            "work",
            "--repo",
            "app"
        ])
        .is_err());
    }

    #[test]
    fn test_workspace_add_with_repo_flags() {
        let args = ws_add(parse(&[
            "workspace",
            "add",
            "my-session",
            "--repo",
            "app-a",
            "--repo",
            "app-b",
        ]));
        assert_eq!(args.repo, vec!["app-a", "app-b"]);
    }

    #[test]
    fn test_workspace_add_default_strategy() {
        let args = ws_add(parse(&["workspace", "add", "my-session", "--repo", "app"]));
        assert_eq!(args.strategy, "worktree");
    }

    #[test]
    fn test_workspace_add_with_strategy_clone() {
        let args = ws_add(parse(&[
            "workspace",
            "add",
            "my-session",
            "--repo",
            "app",
            "--strategy",
            "clone",
        ]));
        assert_eq!(args.strategy, "clone");
    }

    #[test]
    fn test_workspace_add_no_fetch_defaults_false() {
        let args = ws_add(parse(&["workspace", "add", "my-session", "--repo", "app"]));
        assert!(!args.no_fetch);
    }

    #[test]
    fn test_workspace_add_with_no_fetch_flag() {
        let args = ws_add(parse(&[
            "workspace",
            "add",
            "my-session",
            "--repo",
            "app",
            "--no-fetch",
        ]));
        assert!(args.no_fetch);
    }

    // -- repo subcommand (workspace/preset repo management) --

    #[test]
    fn test_repo_add_to_workspace() {
        let cli = parse(&["repo", "add", "app-c", "--workspace", "my-session"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Add(args),
            }) => {
                assert_eq!(args.repo, vec!["app-c"]);
                assert_eq!(args.workspace.as_deref(), Some("my-session"));
                assert!(args.preset.is_none());
            }
            other => panic!("expected Repo Add, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_add_multiple_to_preset() {
        let cli = parse(&["repo", "add", "app-c", "app-d", "--preset", "work"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Add(args),
            }) => {
                assert_eq!(args.repo, vec!["app-c", "app-d"]);
                assert_eq!(args.preset.as_deref(), Some("work"));
            }
            other => panic!("expected Repo Add, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_add_requires_repo() {
        assert!(try_parse(&["repo", "add", "--workspace", "my-session"]).is_err());
    }

    #[test]
    fn test_repo_add_workspace_and_preset_conflict() {
        assert!(try_parse(&[
            "repo",
            "add",
            "app",
            "--workspace",
            "ws",
            "--preset",
            "work"
        ])
        .is_err());
    }

    #[test]
    fn test_repo_remove_from_workspace() {
        let cli = parse(&["repo", "remove", "app-a", "--workspace", "my-session"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::Remove(args),
            }) => {
                assert_eq!(args.repo, vec!["app-a"]);
                assert_eq!(args.workspace.as_deref(), Some("my-session"));
            }
            other => panic!("expected Repo Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_remove_alias_rm() {
        let cli = parse(&["repo", "rm", "app-a", "--workspace", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Repo {
                action: RepoAction::Remove(_)
            })
        ));
    }

    #[test]
    fn test_repo_list_workspace() {
        let cli = parse(&["repo", "list", "--workspace", "my-session"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::List(args),
            }) => assert_eq!(args.workspace.as_deref(), Some("my-session")),
            other => panic!("expected Repo List, got {:?}", other),
        }
    }

    #[test]
    fn test_repo_list_alias_ls() {
        let cli = parse(&["repo", "ls", "--preset", "work"]);
        match cli.command {
            Some(Commands::Repo {
                action: RepoAction::List(args),
            }) => assert_eq!(args.preset.as_deref(), Some("work")),
            other => panic!("expected Repo List, got {:?}", other),
        }
    }

    #[test]
    fn test_workspace_cannot_remove_final_repo() {
        assert!(ensure_workspace_has_repos("my-session", &[]).is_err());
        assert!(ensure_workspace_has_repos("my-session", &["app".to_string()]).is_ok());
    }

    // -- workspace remove subcommand --

    fn ws_remove(cli: Cli) -> RemoveArgs {
        match cli.command {
            Some(Commands::Workspace {
                action: WorkspaceAction::Remove(args),
            }) => args,
            other => panic!("expected Workspace Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_parses() {
        let args = ws_remove(parse(&["workspace", "remove", "my-session"]));
        assert_eq!(args.name.as_deref(), Some("my-session"));
        assert!(!args.all);
    }

    #[test]
    fn test_remove_alias_rm() {
        let args = ws_remove(parse(&["workspace", "rm", "my-session"]));
        assert_eq!(args.name.as_deref(), Some("my-session"));
    }

    #[test]
    fn test_remove_no_name_prunes_after_one_day_by_default() {
        let args = ws_remove(parse(&["workspace", "remove"]));
        assert!(args.name.is_none());
        assert!(!args.all);
        assert!(args.older_than.is_none());
    }

    #[test]
    fn test_remove_older_than_parses() {
        let args = ws_remove(parse(&["workspace", "remove", "--older-than", "7d"]));
        assert!(args.name.is_none());
        assert_eq!(args.older_than.as_deref(), Some("7d"));
    }

    #[test]
    fn test_remove_older_than_conflicts_with_name_and_all() {
        assert!(try_parse(&["workspace", "remove", "my-session", "--older-than", "7d"]).is_err());
        assert!(try_parse(&["workspace", "remove", "--all", "--older-than", "7d"]).is_err());
    }

    #[test]
    fn test_remove_all_flag_parses() {
        let args = ws_remove(parse(&["workspace", "remove", "--all"]));
        assert!(args.name.is_none());
        assert!(args.all);
    }

    #[test]
    fn test_remove_all_short_flag_parses() {
        let args = ws_remove(parse(&["workspace", "rm", "-a"]));
        assert!(args.all);
    }

    #[test]
    fn test_remove_all_conflicts_with_name() {
        assert!(try_parse(&["workspace", "remove", "--all", "my-session"]).is_err());
    }

    #[test]
    fn test_remove_rejects_unknown_flags() {
        assert!(try_parse(&["workspace", "remove", "my-session", "-d"]).is_err());
    }

    // -- workspace switch subcommand --

    #[test]
    fn test_switch_subcommand_parses() {
        let cli = parse(&["workspace", "switch", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Workspace {
                action: WorkspaceAction::Switch { ref name }
            }) if name == "my-session"
        ));
    }

    #[test]
    fn test_switch_alias_sw() {
        let cli = parse(&["workspace", "sw", "my-session"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Workspace {
                action: WorkspaceAction::Switch { ref name }
            }) if name == "my-session"
        ));
    }

    #[test]
    fn test_switch_requires_name() {
        assert!(try_parse(&["workspace", "switch"]).is_err());
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

    // -- workspace list subcommand --

    fn ws_list(cli: Cli) -> ListArgs {
        match cli.command {
            Some(Commands::Workspace {
                action: WorkspaceAction::List(args),
            }) => args,
            other => panic!("expected Workspace List, got {:?}", other),
        }
    }

    #[test]
    fn test_list_no_flags() {
        assert!(!ws_list(parse(&["workspace", "list"])).quiet);
    }

    #[test]
    fn test_list_quiet_flag() {
        assert!(ws_list(parse(&["workspace", "list", "-q"])).quiet);
    }

    #[test]
    fn test_list_alias_ls() {
        assert!(!ws_list(parse(&["workspace", "ls"])).quiet);
    }

    #[test]
    fn test_list_rejects_positional_args() {
        assert!(try_parse(&["workspace", "list", "my-session"]).is_err());
    }

    // -- source subcommand (registry) --

    #[test]
    fn test_source_add_requires_path() {
        assert!(try_parse(&["source", "add"]).is_err());
    }

    #[test]
    fn test_source_add_with_path() {
        let cli = parse(&["source", "add", "/tmp/my-repo"]);
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Add { src },
            }) => assert_eq!(src, "/tmp/my-repo"),
            other => panic!("expected Source Add, got {:?}", other),
        }
    }

    #[test]
    fn test_source_add_with_url() {
        let cli = parse(&["source", "add", "git@github.com:user/app.git"]);
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Add { src },
            }) => assert_eq!(src, "git@github.com:user/app.git"),
            other => panic!("expected Source Add, got {:?}", other),
        }
    }

    #[test]
    fn test_source_remove() {
        let cli = parse(&["source", "remove", "my-app"]);
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Remove { name },
            }) => assert_eq!(name, "my-app"),
            other => panic!("expected Source Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_source_remove_alias_rm() {
        let cli = parse(&["source", "rm", "my-app"]);
        match cli.command {
            Some(Commands::Source {
                action: SourceAction::Remove { name },
            }) => assert_eq!(name, "my-app"),
            other => panic!("expected Source Remove, got {:?}", other),
        }
    }

    #[test]
    fn test_source_list() {
        assert!(matches!(
            parse(&["source", "list"]).command,
            Some(Commands::Source {
                action: SourceAction::List
            })
        ));
    }

    #[test]
    fn test_source_list_alias_ls() {
        assert!(matches!(
            parse(&["source", "ls"]).command,
            Some(Commands::Source {
                action: SourceAction::List
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
    fn test_preset_add_requires_repo() {
        assert!(try_parse(&["preset", "add", "work"]).is_err());
    }

    #[test]
    fn test_preset_update_parses() {
        let cli = parse(&[
            "preset", "update", "work", "--repo", "app-a", "--repo", "app-c",
        ]);
        match cli.command {
            Some(Commands::Preset {
                action: PresetAction::Update { name, repo },
            }) => {
                assert_eq!(name, "work");
                assert_eq!(repo, vec!["app-a", "app-c"]);
            }
            other => panic!("expected Preset Update, got {:?}", other),
        }
    }

    #[test]
    fn test_preset_update_requires_repo() {
        assert!(try_parse(&["preset", "update", "work"]).is_err());
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

    // -- prune age parsing --

    #[test]
    fn test_parse_age_supports_common_units() {
        assert_eq!(parse_age("30s").unwrap(), chrono::Duration::seconds(30));
        assert_eq!(parse_age("15m").unwrap(), chrono::Duration::minutes(15));
        assert_eq!(parse_age("12h").unwrap(), chrono::Duration::hours(12));
        assert_eq!(parse_age("7d").unwrap(), chrono::Duration::days(7));
    }

    #[test]
    fn test_parse_age_rejects_invalid_values() {
        for value in ["", "1", "0d", "-1d", "day", "9223372036854775807d"] {
            assert!(parse_age(value).is_err(), "{value} should be rejected");
        }
    }

    #[test]
    fn test_prune_selection_respects_age_active_workspace_and_missing_metadata() {
        let cutoff = chrono::Utc::now();
        let old = Some(cutoff - chrono::Duration::seconds(1));
        let new = Some(cutoff + chrono::Duration::seconds(1));

        let mut active = std::collections::BTreeSet::new();
        assert!(is_prunable("old", old, cutoff, &active));
        assert!(is_prunable("boundary", Some(cutoff), cutoff, &active));
        assert!(!is_prunable("new", new, cutoff, &active));
        assert!(!is_prunable("unknown", None, cutoff, &active));

        active.insert("old".to_string());
        assert!(!is_prunable("old", old, cutoff, &active));
    }

    // -- no --update flag (removed) --

    #[test]
    fn test_update_flag_rejected() {
        let result = try_parse(&["--update"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_update_flag_rejected() {
        let result = try_parse(&[
            "workspace",
            "add",
            "my-session",
            "--repo",
            "app",
            "--update",
        ]);
        assert!(result.is_err());
    }

    // -- bare name is rejected (subcommand required) --

    #[test]
    fn test_bare_name_rejected() {
        let result = try_parse(&["my-session"]);
        assert!(result.is_err());
    }
}
