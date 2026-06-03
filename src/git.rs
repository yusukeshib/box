use std::path::Path;

pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Walk up from `dir` to find the nearest ancestor containing `.git`.
pub fn find_root(dir: &Path) -> Option<&Path> {
    let mut current = dir;
    loop {
        if is_repo(current) {
            return Some(current);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

/// Fetch all refs for a bare repo, capturing output.
///
/// Uses `--prune` so local heads whose upstream counterpart was deleted are
/// removed, keeping the bare repo in sync with `origin`. Branches checked out
/// in a worktree are excluded from the refspec, so they are never pruned.
///
/// When worktrees have branches checked out, git refuses to update those refs
/// via fetch. We detect checked-out branches and exclude them with negative
/// refspecs. If git still refuses (e.g. a worktree admin entry we missed),
/// we parse the error, add the offending branch to the excludes, and retry.
///
/// Returns (success, captured_output) for use in parallel execution.
pub fn fetch_repo(entry: &crate::repo::RepoEntry) -> (bool, String) {
    let mut excludes = worktree_checked_out_branches(&entry.path);
    let mut log = String::new();

    for _ in 0..8 {
        let mut args: Vec<String> = vec![
            "-C".into(),
            entry.path.clone(),
            "fetch".into(),
            "--prune".into(),
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

    #[test]
    fn test_find_root_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", tmp.path().to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(find_root(tmp.path()), Some(tmp.path()));
    }

    #[test]
    fn test_find_root_from_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", tmp.path().to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let sub = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_root(&sub), Some(tmp.path()));
    }

    #[test]
    fn test_find_root_no_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("no_repo");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_root(&sub), None);
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
