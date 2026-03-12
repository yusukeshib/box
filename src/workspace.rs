use anyhow::{bail, Result};
use std::process::Command;

use crate::config;

/// Remove the workspace directory for a session.
pub fn remove_workspace(name: &str) {
    if let Ok(root) = config::box_root() {
        let dir = root.join("workspaces").join(name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Create a multi-repo workspace. Each repo is cloned into a subdirectory
/// of `~/.box/workspaces/<session>/`. Returns the session workspace root path.
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

    for repo in repos {
        let dest = root.join(&repo.name);
        let dest_str = dest.to_string_lossy().to_string();
        if !dest.join(".git").exists() {
            eprintln!("\x1b[2mcloning {}:\x1b[0m", repo.name);
            let status = Command::new("git")
                .args(["clone", "--local", &repo.path, &dest_str])
                .current_dir(&root)
                .status()?;
            if !status.success() {
                bail!("git clone --local failed for '{}'", repo.name);
            }
            repoint_origin(&repo.path, &dest_str);
        }
    }

    Ok(root.to_string_lossy().to_string())
}

/// Remove a single repo subdirectory from a session workspace.
pub fn remove_repo_from_workspace(session_name: &str, repo_name: &str) {
    if let Ok(root) = config::box_root() {
        let dir = root.join("workspaces").join(session_name).join(repo_name);
        let _ = std::fs::remove_dir_all(&dir);
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
                    .status();
            }
        }
    }
}
