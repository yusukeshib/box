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
                .output();
            match result {
                Ok(output) => {
                    let mut buf = String::from_utf8_lossy(&output.stderr).to_string();
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

        let mut failures = Vec::new();
        for result in &results {
            eprintln!("\x1b[2mcloning {}:\x1b[0m", result.name);
            if !result.output.is_empty() {
                eprint!("{}", result.output);
            }
            if !result.success {
                failures.push(result.name.clone());
            }
        }
        if !failures.is_empty() {
            bail!("git clone --local failed for: {}", failures.join(", "));
        }
    }

    Ok(root.to_string_lossy().to_string())
}

/// Create a multi-repo workspace using git worktree add.
pub fn ensure_workspace_multi_worktree(
    session_name: &str,
    repos: &[crate::repo::RepoEntry],
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
                    .output();

                match result {
                    Ok(output) if output.status.success() => {
                        (true, String::from_utf8_lossy(&output.stderr).to_string())
                    }
                    Ok(_) => {
                        // Branch may already exist from a partial retry; try without -b
                        match Command::new("git")
                            .args(["-C", &repo.path, "worktree", "add", &dest_str, &branch])
                            .output()
                        {
                            Ok(output2) => {
                                let buf = String::from_utf8_lossy(&output2.stderr).to_string();
                                (output2.status.success(), buf)
                            }
                            Err(e) => (false, format!("failed to run git: {}\n", e)),
                        }
                    }
                    Err(e) => (false, format!("failed to run git: {}\n", e)),
                }
            });

        let mut failures = Vec::new();
        for result in &results {
            eprintln!("\x1b[2mworktree {}:\x1b[0m", result.name);
            if !result.output.is_empty() {
                eprint!("{}", result.output);
            }
            if !result.success {
                failures.push(result.name.clone());
            }
        }
        if !failures.is_empty() {
            bail!("git worktree add failed for: {}", failures.join(", "));
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
pub fn remove_workspace_worktree(name: &str, repo_names: &[String]) {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = match config::box_root() {
        Ok(r) => r,
        Err(_) => return,
    };
    let branch_name = format!("box/{}", name);

    for repo_name in repo_names {
        let dest = root.join("workspaces").join(name).join(repo_name);
        let dest_str = dest.to_string_lossy().to_string();

        if let Some(entry) = all_repos.iter().find(|r| r.name == *repo_name) {
            // Remove worktree via git
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
            // Delete the branch
            let _ = Command::new("git")
                .args(["-C", &entry.path, "branch", "-D", &branch_name])
                .status();
        } else {
            // Repo not in registry, fall back to rm -rf
            let _ = std::fs::remove_dir_all(&dest);
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
) -> Result<String> {
    match strategy {
        Strategy::Clone => ensure_workspace_multi(name, repos),
        Strategy::Worktree => ensure_workspace_multi_worktree(name, repos),
    }
}

/// Remove workspace using the given strategy.
pub fn remove_workspace_by_strategy(name: &str, repo_names: &[String], strategy: Strategy) {
    match strategy {
        Strategy::Clone => remove_workspace(name),
        Strategy::Worktree => remove_workspace_worktree(name, repo_names),
    }
}

/// Remove a single repo from workspace using the given strategy.
pub fn remove_repo_by_strategy(session_name: &str, repo_name: &str, strategy: Strategy) {
    match strategy {
        Strategy::Clone => remove_repo_from_workspace(session_name, repo_name),
        Strategy::Worktree => remove_repo_from_workspace_worktree(session_name, repo_name),
    }
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
