use std::path::Path;

pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
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
