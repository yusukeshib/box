use std::path::Path;
use std::process::Command;

pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").is_dir()
}

pub fn reset_index(project_dir: &str) {
    let _ = Command::new("git")
        .args(["-C", project_dir, "reset", "--quiet"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn init_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        StdCommand::new("git")
            .args(["init", tmp.path().to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        StdCommand::new("git")
            .args([
                "-C",
                tmp.path().to_str().unwrap(),
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        tmp
    }

    #[test]
    fn test_is_repo_true() {
        let tmp = init_git_repo();
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
    fn test_reset_index() {
        let tmp = init_git_repo();
        let dir = tmp.path().to_str().unwrap();
        // Should not panic even with nothing to reset
        reset_index(dir);
    }

    #[test]
    fn test_reset_index_nonexistent_dir() {
        // Should not panic
        reset_index("/nonexistent/dir/12345");
    }
}
