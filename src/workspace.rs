use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

use crate::config;

/// Create a workspace directory on the host for the session.
/// Dispatches to clone or worktree strategy based on the `strategy` parameter.
/// Returns the host path.
pub fn ensure_workspace(
    home: &str,
    name: &str,
    project_dir: &str,
    strategy: &crate::config::Strategy,
) -> Result<String> {
    match strategy {
        crate::config::Strategy::Worktree => ensure_workspace_worktree(home, name, project_dir),
        crate::config::Strategy::Clone => ensure_workspace_clone(home, name, project_dir),
    }
}

/// Create a workspace via `git clone --local`.
/// Returns the host path.
fn ensure_workspace_clone(home: &str, name: &str, project_dir: &str) -> Result<String> {
    let dir_path = Path::new(home).join(".box").join("workspaces").join(name);
    let dir = dir_path.to_string_lossy().to_string();
    let git_dir = dir_path.join(".git");

    if !Path::new(&git_dir).exists() {
        eprintln!("\x1b[2mrunning clone command:\x1b[0m");
        eprintln!("git clone --local {} {}", project_dir, dir);
        let status = Command::new("git")
            .args(["clone", "--local", project_dir, &dir])
            .status()?;
        if !status.success() {
            bail!("git clone --local failed");
        }

        // git clone --local sets origin to the host path. Re-point origin
        // to the real remote URL so fetches work from the workspace.
        if let Ok(output) = Command::new("git")
            .args(["-C", project_dir, "remote", "get-url", "origin"])
            .output()
        {
            if output.status.success() {
                let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !url.is_empty() {
                    eprintln!("\x1b[2mrunning remote update:\x1b[0m");
                    eprintln!("git remote set-url origin {}", url);
                    let _ = Command::new("git")
                        .args(["-C", &dir, "remote", "set-url", "origin", &url])
                        .status();
                }
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir)?.permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&dir, perms)?;
    }

    Ok(dir)
}

/// Create a workspace via `git worktree add --detach`.
fn ensure_workspace_worktree(home: &str, name: &str, project_dir: &str) -> Result<String> {
    let dir_path = Path::new(home).join(".box").join("workspaces").join(name);
    let dir = dir_path.to_string_lossy().to_string();

    if !dir_path.exists() {
        eprintln!("\x1b[2mrunning worktree command:\x1b[0m");
        eprintln!("git -C {} worktree add --detach {}", project_dir, dir);
        let status = Command::new("git")
            .args(["-C", project_dir, "worktree", "add", "--detach", &dir])
            .status()?;
        if !status.success() {
            bail!("git worktree add failed");
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir)?.permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&dir, perms)?;
    }

    Ok(dir)
}

/// Remove the workspace for a session. Dispatches based on strategy.
pub fn remove_workspace(name: &str, strategy: &crate::config::Strategy) {
    match strategy {
        crate::config::Strategy::Worktree => remove_workspace_worktree(name),
        crate::config::Strategy::Clone => remove_workspace_clone(name),
    }
}

/// Remove the workspace directory for a clone-based session.
fn remove_workspace_clone(name: &str) {
    if let Ok(home) = config::home_dir() {
        let dir = Path::new(&home).join(".box").join("workspaces").join(name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Remove the workspace for a worktree-based session.
fn remove_workspace_worktree(name: &str) {
    if let Ok(home) = config::home_dir() {
        let dir = Path::new(&home).join(".box").join("workspaces").join(name);
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", &dir.to_string_lossy()])
            .status();
    }
}
