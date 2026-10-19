use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            temp: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn workspace(&self, name: &str) -> PathBuf {
        self.root().join("workspaces").join(name)
    }

    fn add(&self, name: &str) {
        let metadata = self.root().join("sessions").join(name);
        fs::create_dir_all(&metadata).unwrap();
        let project = self.root().join(format!("project-{name}"));
        fs::create_dir_all(&project).unwrap();
        fs::write(metadata.join("project_dir"), project.to_str().unwrap()).unwrap();
        fs::write(metadata.join("repos"), "repo\n").unwrap();
        fs::write(metadata.join("strategy"), "clone").unwrap();
        fs::create_dir_all(self.workspace(name).join("repo/subdir")).unwrap();
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_box"));
        cmd.env("BOX_ROOT", self.root())
            .env("BOX_CD_FILE", self.root().join("cd"))
            .current_dir(self.root());
        cmd
    }

    fn remove(&self, names: &[&str]) -> Output {
        self.command()
            .args(["workspace", "rm"])
            .args(names)
            .output()
            .unwrap()
    }

    fn exists(&self, name: &str) -> bool {
        self.workspace(name).exists() && self.root().join("sessions").join(name).exists()
    }
}

#[test]
fn removes_multiple_names_once_and_leaves_other_workspaces() {
    let f = Fixture::new();
    for name in ["a", "b", "c", "d", "keep"] {
        f.add(name);
    }
    let output = f.remove(&["a", "b", "a", "a-", "c", "d"]);
    assert!(output.status.success(), "{output:?}");
    for name in ["a", "b", "c", "d"] {
        assert!(!f.workspace(name).exists());
        assert!(!f.root().join("sessions").join(name).exists());
    }
    assert!(f.exists("keep"));
    assert!(!f.root().join("cd").exists());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches("Session 'a' removed.")
            .count(),
        1
    );
}

#[test]
fn validates_every_target_before_deleting() {
    for invalid in ["missing", ".."] {
        let f = Fixture::new();
        f.add("a");
        assert!(!f.remove(&["a", invalid]).status.success());
        assert!(f.exists("a"));
        assert!(!f.root().join("cd").exists());
    }
}

#[test]
fn deletion_failure_retains_metadata_and_removes_other_targets() {
    let f = Fixture::new();
    f.add("a");
    f.add("b");
    fs::remove_dir_all(f.workspace("b")).unwrap();
    // A file where a workspace directory should be produces a deterministic
    // deletion failure even when tests run with elevated permissions.
    fs::write(f.workspace("b"), "not a directory").unwrap();
    let output = f.remove(&["a", "b"]);
    assert!(!output.status.success());
    assert!(!f.workspace("a").exists());
    assert!(!f.root().join("sessions/a").exists());
    assert!(f.root().join("sessions/b").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("metadata was retained"));
}

#[test]
fn single_removal_still_signals_project_directory() {
    let f = Fixture::new();
    f.add("a");
    assert!(f.remove(&["a"]).status.success());
    assert_eq!(
        fs::read_to_string(f.root().join("cd")).unwrap(),
        f.root().join("project-a").to_str().unwrap()
    );
}

#[test]
fn shell_wrappers_return_from_removed_workspace_in_both_shells() {
    for shell in ["bash", "zsh"] {
        match Command::new(shell).arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("Skipping {shell} wrapper test: shell is not installed");
                continue;
            }
            Err(e) => panic!("Cannot run {shell}: {e}"),
            Ok(_) => {}
        }
        let f = Fixture::new();
        f.add("a");
        f.add("b");
        let script = format!(
            "{}\neval \"$(\"$BOX_TEST_BINARY\" config {shell})\"\nbox ws rm a b || exit $?\nprintf '%s' \"$PWD\" > \"$BOX_ROOT/result\"",
            if shell == "zsh" { "autoload -Uz compinit; compinit -D" } else { ":" }
        );
        let output = Command::new(shell)
            .args(["-f", "-c", &script])
            .env("BOX_ROOT", f.root())
            .env("BOX_TEST_BINARY", env!("CARGO_BIN_EXE_box"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    Path::new(env!("CARGO_BIN_EXE_box"))
                        .parent()
                        .unwrap()
                        .display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env_remove("BOX_POST_SWITCH_HOOK")
            .current_dir(f.workspace("b").join("repo/subdir"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{shell}: {output:?}");
        assert_eq!(
            fs::read_to_string(f.root().join("result")).unwrap(),
            f.root().join("project-b").to_str().unwrap(),
            "{shell}"
        );
        assert!(!f.workspace("a").exists());
        assert!(!f.workspace("b").exists());
    }
}
