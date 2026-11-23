use std::path::Path;

pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Fetch `origin` into a bare repo, capturing output.
///
/// Remote state lives only in `refs/remotes/origin/*`, pruned with the remote.
/// `refs/heads/*` holds local branches: every workspace's session branch and
/// anything users or tools such as Graphite create there, checked out or not.
/// Fetch never mirrors or prunes them, so unpushed work cannot be overwritten
/// or deleted. The one local branch it moves is the default branch named by
/// the bare's `HEAD`, which new workspaces start from: it is fast-forwarded
/// to `origin/<default>` unless a worktree has it checked out or it has
/// diverged.
///
/// Returns (success, captured_output) for use in parallel execution.
pub fn fetch_repo(entry: &crate::repo::RepoEntry) -> (bool, String) {
    let mut log = String::new();

    match std::process::Command::new("git")
        .args([
            "-C",
            &entry.path,
            "fetch",
            "--prune",
            // Use exactly this refspec, whatever the bare's config says.
            "--refmap=",
            "origin",
            crate::repo::REFSPEC_ORIGIN,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(output) => {
            log.push_str(&String::from_utf8_lossy(&output.stdout));
            log.push_str(&String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                log.push_str("  \x1b[31mfetch failed\x1b[0m\n");
                return (false, log);
            }
        }
        Err(e) => {
            log.push_str(&format!("  \x1b[31mfetch error: {}\x1b[0m\n", e));
            return (false, log);
        }
    }

    fast_forward_default_branch(&entry.path, &mut log);
    (true, log)
}

/// Fast-forward the bare's default branch (its `HEAD`) to `origin/<default>`.
/// Leaves it alone when it is checked out in a worktree or has commits that
/// `origin` lacks; creates it when missing.
fn fast_forward_default_branch(bare: &str, log: &mut String) {
    let Some(head) = git_output(bare, &["symbolic-ref", "--quiet", "HEAD"]) else {
        return;
    };
    let Some(branch) = head.strip_prefix("refs/heads/") else {
        return;
    };
    let remote_ref = format!("refs/remotes/origin/{}^{{commit}}", branch);
    let Some(remote) = git_output(bare, &["rev-parse", "--verify", "--quiet", &remote_ref]) else {
        return;
    };
    let local = git_output(bare, &["rev-parse", "--verify", "--quiet", &head]);
    if local.as_deref() == Some(remote.as_str()) {
        return;
    }
    if worktree_checked_out_branches(bare)
        .iter()
        .any(|b| b == branch)
    {
        return;
    }
    if let Some(local) = &local {
        let ancestor = std::process::Command::new("git")
            .args(["-C", bare, "merge-base", "--is-ancestor", local, &remote])
            .status()
            .is_ok_and(|s| s.success());
        if !ancestor {
            log.push_str(&format!(
                "  \x1b[33m{} has commits origin lacks; not fast-forwarded\x1b[0m\n",
                branch
            ));
            return;
        }
    }
    // The old value makes the update atomic against concurrent writers.
    let old = local.unwrap_or_default();
    let updated = std::process::Command::new("git")
        .args(["-C", bare, "update-ref", &head, &remote, &old])
        .output()
        .is_ok_and(|o| o.status.success());
    if !updated {
        log.push_str(&format!(
            "  \x1b[33mcould not fast-forward {}\x1b[0m\n",
            branch
        ));
    }
}

/// Trimmed stdout of a successful git command in `dir`.
fn git_output(dir: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
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

/// The ref a new session branch named `branch` should start from: the
/// remote branch of that name when one exists, so a workspace named after an
/// existing branch picks it up; otherwise `None` (the bare's `HEAD`).
pub fn session_start_point(bare: &str, branch: &str) -> Option<String> {
    let remote = format!("refs/remotes/origin/{}", branch);
    git_output(
        bare,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", remote),
        ],
    )
    .map(|_| remote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_repo_true() {
        let tmp = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", tmp.path().to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(is_repo(tmp.path()));
    }

    #[test]
    fn test_is_repo_git_file() {
        // Worktrees and submodules use a .git file instead of a directory
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".git"), "gitdir: /some/path").unwrap();
        assert!(is_repo(tmp.path()));
    }

    #[test]
    fn test_is_repo_false() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_repo(tmp.path()));
    }

    #[test]
    fn test_is_repo_nonexistent() {
        assert!(!is_repo(Path::new("/nonexistent/path/12345")));
    }
}
