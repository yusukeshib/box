use anyhow::{bail, Result};
use std::fmt;
use std::process::Command;

use crate::config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    Clone,
    Worktree,
}

impl Strategy {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "clone" => Ok(Strategy::Clone),
            "worktree" => Ok(Strategy::Worktree),
            other => bail!("Unknown strategy '{}'. Use 'clone' or 'worktree'.", other),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Strategy::Clone => "clone",
            Strategy::Worktree => "worktree",
        }
    }

    /// Resolve strategy from an optional CLI value, falling back to BOX_STRATEGY env var,
    /// then defaulting to "worktree".
    pub fn resolve(cli_value: Option<&str>) -> Result<Self> {
        let value = match cli_value {
            Some(v) => v.to_string(),
            None => std::env::var("BOX_STRATEGY").unwrap_or_else(|_| "worktree".to_string()),
        };
        Strategy::from_str(&value)
    }
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Remove the workspace directory for a session (clone strategy).
pub fn remove_workspace(name: &str) {
    if let Ok(root) = config::box_root() {
        let dir = root.join("workspaces").join(name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Create a multi-repo workspace using git clone --local.
pub fn ensure_workspace_multi(
    session_name: &str,
    repos: &[crate::repo::RepoEntry],
    verbose: bool,
) -> Result<String> {
    let root = config::box_root()?.join("workspaces").join(session_name);
    std::fs::create_dir_all(&root)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&root)?.permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&root, perms)?;
    }

    let to_clone: Vec<(String, (crate::repo::RepoEntry, String))> = repos
        .iter()
        .filter(|repo| !root.join(&repo.name).join(".git").exists())
        .map(|repo| {
            let dest_str = root.join(&repo.name).to_string_lossy().to_string();
            (repo.name.clone(), (repo.clone(), dest_str))
        })
        .collect();

    if !to_clone.is_empty() {
        let root_str = root.to_string_lossy().to_string();
        let results = crate::parallel::run_parallel(to_clone, move |_name, (repo, dest_str)| {
            let result = Command::new("git")
                .args(["clone", "--local", &repo.path, &dest_str])
                .current_dir(&root_str)
                .env("GIT_TERMINAL_PROMPT", "0")
                .output();
            match result {
                Ok(output) => {
                    let mut buf = captured_output(&output);
                    if output.status.success() {
                        repoint_origin(&repo.path, &dest_str);
                        (true, buf)
                    } else {
                        buf.push_str(&format!("git clone --local failed for '{}'\n", repo.name));
                        (false, buf)
                    }
                }
                Err(e) => (false, format!("failed to run git: {}\n", e)),
            }
        });

        let mut failure_msgs = Vec::new();
        if verbose {
            for result in &results {
                eprintln!("\x1b[2mcloning {}:\x1b[0m", result.name);
                if !result.output.is_empty() {
                    eprint!("{}", result.output);
                }
                if !result.success {
                    failure_msgs.push(result.name.clone());
                }
            }
        } else {
            for result in &results {
                if !result.success {
                    failure_msgs.push(format!("  {}: {}", result.name, result.output.trim()));
                }
            }
        }
        if !failure_msgs.is_empty() {
            if verbose {
                bail!("git clone --local failed for: {}", failure_msgs.join(", "));
            } else {
                bail!("git clone --local failed:\n{}", failure_msgs.join("\n"));
            }
        }
    }

    Ok(root.to_string_lossy().to_string())
}

/// Create a multi-repo workspace using git worktree add.
pub fn ensure_workspace_multi_worktree(
    session_name: &str,
    repos: &[crate::repo::RepoEntry],
    verbose: bool,
) -> Result<String> {
    let root = config::box_root()?.join("workspaces").join(session_name);
    std::fs::create_dir_all(&root)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&root)?.permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&root, perms)?;
    }

    let branch_name = format!("box/{}", session_name);

    let to_create: Vec<(String, (crate::repo::RepoEntry, String, String))> = repos
        .iter()
        .filter(|repo| !root.join(&repo.name).join(".git").exists())
        .map(|repo| {
            let dest_str = root.join(&repo.name).to_string_lossy().to_string();
            (
                repo.name.clone(),
                (repo.clone(), dest_str, branch_name.clone()),
            )
        })
        .collect();

    if !to_create.is_empty() {
        let results =
            crate::parallel::run_parallel(to_create, |_name, (repo, dest_str, branch)| {
                // Try with -b first to create a new branch
                let result = Command::new("git")
                    .args([
                        "-C", &repo.path, "worktree", "add", &dest_str, "-b", &branch,
                    ])
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .output();

                match result {
                    Ok(output) if output.status.success() => (true, captured_output(&output)),
                    Ok(first_output) => {
                        let first_err = captured_output(&first_output);
                        // Branch may already exist from a partial retry; try without -b
                        match Command::new("git")
                            .args(["-C", &repo.path, "worktree", "add", &dest_str, &branch])
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .output()
                        {
                            Ok(output2) => {
                                let mut buf = first_err;
                                buf.push_str(&captured_output(&output2));
                                (output2.status.success(), buf)
                            }
                            Err(e) => {
                                let mut buf = first_err;
                                buf.push_str(&format!("failed to run git: {}\n", e));
                                (false, buf)
                            }
                        }
                    }
                    Err(e) => (false, format!("failed to run git: {}\n", e)),
                }
            });

        let mut failure_msgs = Vec::new();
        if verbose {
            for result in &results {
                eprintln!("\x1b[2mworktree {}:\x1b[0m", result.name);
                if !result.output.is_empty() {
                    eprint!("{}", result.output);
                }
                if !result.success {
                    failure_msgs.push(result.name.clone());
                }
            }
        } else {
            for result in &results {
                if !result.success {
                    failure_msgs.push(format!("  {}: {}", result.name, result.output.trim()));
                }
            }
        }
        if !failure_msgs.is_empty() {
            if verbose {
                bail!("git worktree add failed for: {}", failure_msgs.join(", "));
            } else {
                bail!("git worktree add failed:\n{}", failure_msgs.join("\n"));
            }
        }
    }

    Ok(root.to_string_lossy().to_string())
}

/// Remove a single repo subdirectory from a session workspace (clone strategy).
pub fn remove_repo_from_workspace(session_name: &str, repo_name: &str) {
    if let Ok(root) = config::box_root() {
        let dir = root.join("workspaces").join(session_name).join(repo_name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Remove worktrees for a session. For each repo, removes the worktree and
/// deletes the branch. Falls back to rm -rf if repo not in registry.
/// Repos are removed in parallel for speed.
pub fn remove_workspace_worktree(name: &str, repo_names: &[String], verbose: bool) {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = match config::box_root() {
        Ok(r) => r,
        Err(_) => return,
    };
    let branch_name = format!("box/{}", name);

    let items: Vec<_> = repo_names
        .iter()
        .map(|repo_name| {
            let dest = root.join("workspaces").join(name).join(repo_name);
            let repo_path = all_repos
                .iter()
                .find(|r| r.name == *repo_name)
                .map(|r| r.path.clone());
            (repo_name.clone(), (repo_path, dest, branch_name.clone()))
        })
        .collect();

    if !items.is_empty() {
        let count = items.len();
        if !verbose {
            eprint!(
                "\x1b[2mRemoving {} worktree{}…\x1b[0m ",
                count,
                if count == 1 { "" } else { "s" }
            );
        }
        let results = crate::parallel::run_parallel(items, |_name, (repo_path, dest, branch)| {
            if let Some(path) = repo_path {
                let dest_str = dest.to_string_lossy().to_string();
                // Remove worktree via git
                let wt = Command::new("git")
                    .args(["-C", &path, "worktree", "remove", "--force", &dest_str])
                    .output();
                // Delete the branch
                let br = Command::new("git")
                    .args(["-C", &path, "branch", "-D", &branch])
                    .output();
                let mut buf = String::new();
                let mut success = true;
                match wt {
                    Ok(o) => {
                        buf.push_str(&captured_output(&o));
                        if !o.status.success() {
                            success = false;
                        }
                    }
                    Err(e) => {
                        success = false;
                        buf.push_str(&format!("failed to run git worktree remove: {}\n", e));
                    }
                }
                match br {
                    Ok(o) => {
                        buf.push_str(&captured_output(&o));
                        if !o.status.success() {
                            success = false;
                        }
                    }
                    Err(e) => {
                        success = false;
                        buf.push_str(&format!("failed to run git branch -D: {}\n", e));
                    }
                }
                (success, buf)
            } else {
                // Repo not in registry, fall back to rm -rf
                match std::fs::remove_dir_all(&dest) {
                    Ok(()) => (true, String::new()),
                    Err(e) => (
                        false,
                        format!("failed to remove '{}': {}", dest.display(), e),
                    ),
                }
            }
        });

        if verbose {
            for result in &results {
                eprintln!("\x1b[2mremove {}:\x1b[0m", result.name);
                if !result.output.is_empty() {
                    eprint!("{}", result.output);
                }
                if result.success {
                    eprintln!("  \x1b[32mok\x1b[0m");
                } else {
                    eprintln!("  \x1b[31mfailed\x1b[0m");
                }
            }
        } else {
            let failures: Vec<_> = results.iter().filter(|r| !r.success).collect();
            if failures.is_empty() {
                eprintln!("\x1b[32mok\x1b[0m");
            } else {
                eprintln!("\x1b[31m{} failed\x1b[0m", failures.len());
                for f in &failures {
                    eprintln!("  \x1b[1m{}\x1b[0m: {}", f.name, f.output.trim());
                }
            }
        }
    }

    // Remove session workspace root dir
    let session_root = root.join("workspaces").join(name);
    let _ = std::fs::remove_dir_all(&session_root);
}

/// Remove a single repo worktree from a session.
pub fn remove_repo_from_workspace_worktree(session_name: &str, repo_name: &str) {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = match config::box_root() {
        Ok(r) => r,
        Err(_) => return,
    };
    let branch_name = format!("box/{}", session_name);
    let dest = root.join("workspaces").join(session_name).join(repo_name);
    let dest_str = dest.to_string_lossy().to_string();

    if let Some(entry) = all_repos.iter().find(|r| r.name == repo_name) {
        let _ = Command::new("git")
            .args([
                "-C",
                &entry.path,
                "worktree",
                "remove",
                "--force",
                &dest_str,
            ])
            .status();
        let _ = Command::new("git")
            .args(["-C", &entry.path, "branch", "-D", &branch_name])
            .status();
    } else {
        let _ = std::fs::remove_dir_all(&dest);
    }
}

// -- Dispatch wrappers --

/// Create workspace using the given strategy.
pub fn ensure_workspace(
    name: &str,
    repos: &[crate::repo::RepoEntry],
    strategy: Strategy,
    verbose: bool,
) -> Result<String> {
    let count = repos.len();
    let label = match strategy {
        Strategy::Clone => "Cloning",
        Strategy::Worktree => "Creating worktrees",
    };
    if !verbose {
        eprint!(
            "\x1b[2m{} for {} repo{}…\x1b[0m ",
            label,
            count,
            if count == 1 { "" } else { "s" }
        );
    }
    let result = match strategy {
        Strategy::Clone => ensure_workspace_multi(name, repos, verbose),
        Strategy::Worktree => ensure_workspace_multi_worktree(name, repos, verbose),
    };
    if !verbose {
        match &result {
            Ok(_) => eprintln!("\x1b[32mok\x1b[0m"),
            Err(_) => eprintln!("\x1b[31mfailed\x1b[0m"),
        }
    }
    result
}

/// Remove workspace using the given strategy.
pub fn remove_workspace_by_strategy(
    name: &str,
    repo_names: &[String],
    strategy: Strategy,
    verbose: bool,
) {
    match strategy {
        Strategy::Clone => remove_workspace(name),
        Strategy::Worktree => remove_workspace_worktree(name, repo_names, verbose),
    }
}

/// Remove a single repo from workspace using the given strategy.
pub fn remove_repo_by_strategy(session_name: &str, repo_name: &str, strategy: Strategy) {
    match strategy {
        Strategy::Clone => remove_repo_from_workspace(session_name, repo_name),
        Strategy::Worktree => remove_repo_from_workspace_worktree(session_name, repo_name),
    }
}

/// Combine stdout and stderr from a process Output into a single string.
fn captured_output(output: &std::process::Output) -> String {
    let mut buf = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        buf.push_str(&stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        buf.push_str(&stderr);
    }
    buf
}

/// Re-point origin remote from local path to the real remote URL.
fn repoint_origin(project_dir: &str, clone_dir: &str) {
    if let Ok(output) = Command::new("git")
        .args(["-C", project_dir, "remote", "get-url", "origin"])
        .output()
    {
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !url.is_empty() {
                let _ = Command::new("git")
                    .args(["-C", clone_dir, "remote", "set-url", "origin", &url])
                    .output();
            }
        }
    }
}
