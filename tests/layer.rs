//! End-to-end tests for the macOS `layer` strategy. They create and attach
//! real APFS disk images with `hdiutil`, so they only run on macOS.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            temp: tempfile::tempdir().unwrap(),
        };
        // A source repo with one commit on main. `box source add <path>`
        // bare-clones it, so it acts as `origin`.
        let src = f.src();
        git(&["-c", "init.defaultBranch=main", "init", "-q", path(&src)]);
        fs::write(src.join("README.md"), "hello\n").unwrap();
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("dir/a.txt"), "a\n").unwrap();
        commit(&src, "init");
        let out = f.box_cmd(&["source", "add", path(&src)]);
        assert!(out.status.success(), "{out:?}");
        // post-checkout hook in the bare repo records its cwd and arguments.
        let hook = f.bare().join("hooks/post-checkout");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\necho \"$PWD $1 $2 $3\" >> '{}'\n",
                f.hook_log().display()
            ),
        )
        .unwrap();
        Command::new("chmod")
            .args(["+x", path(&hook)])
            .status()
            .unwrap();
        f
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("box")
    }

    fn src(&self) -> PathBuf {
        self.temp.path().join("src")
    }

    fn bare(&self) -> PathBuf {
        self.root().join("repos/src.git")
    }

    fn hook_log(&self) -> PathBuf {
        self.temp.path().join("hook.log")
    }

    fn repo_dir(&self, ws: &str) -> PathBuf {
        self.root().join("workspaces").join(ws).join("src")
    }

    fn box_cmd(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_box"))
            .args(args)
            .env("BOX_ROOT", self.root())
            .env("BOX_CD_FILE", self.temp.path().join("cd"))
            .env("BOX_LAYER_REFRESH", "sync")
            .env_remove("BOX_SESSION")
            .env_remove("BOX_STRATEGY")
            .output()
            .unwrap()
    }

    fn add_layer(&self, ws: &str) {
        let out = self.box_cmd(&[
            "workspace",
            "add",
            ws,
            "--repo",
            "src",
            "--strategy",
            "layer",
        ]);
        assert!(out.status.success(), "{out:?}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Detach anything a failed test left mounted so the temp dir can go.
        let mut candidates = Vec::new();
        for parent in [self.root().join("workspaces"), self.root().join("layers")] {
            for entry in fs::read_dir(parent).into_iter().flatten().flatten() {
                for child in fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                    candidates.push(child.path());
                    for grandchild in fs::read_dir(child.path()).into_iter().flatten().flatten() {
                        candidates.push(grandchild.path());
                    }
                }
            }
        }
        for p in candidates.into_iter().filter(|p| is_mount(p)) {
            let _ = Command::new("hdiutil")
                .args(["detach", "-quiet", "-force", path(&p)])
                .status();
        }
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn git(args: &[&str]) -> String {
    let out = Command::new("git").args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) {
    git(&["-C", path(dir), "add", "-A"]);
    git(&[
        "-C",
        path(dir),
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "-m",
        msg,
    ]);
}

fn is_mount(p: &Path) -> bool {
    match (fs::symlink_metadata(p), p.parent().map(fs::metadata)) {
        (Ok(m), Some(Ok(parent))) => m.is_dir() && m.dev() != parent.dev(),
        _ => false,
    }
}

#[test]
fn layer_workspaces_are_isolated_refreshed_remounted_and_removed() {
    let f = Fixture::new();
    let bare = f.bare();

    // -- First workspace builds the base image and mounts a clone of it.
    f.add_layer("ws1");
    let ws1 = f.repo_dir("ws1");
    assert!(is_mount(&ws1), "ws1 repo dir should be a mounted volume");
    assert_eq!(
        fs::read_to_string(ws1.join("README.md")).unwrap(),
        "hello\n"
    );
    assert_eq!(git(&["-C", path(&ws1), "status", "--porcelain"]), "");
    assert_eq!(git(&["-C", path(&ws1), "branch", "--show-current"]), "ws1");
    assert_eq!(
        git(&["-C", path(&ws1), "config", "branch.ws1.merge"]),
        "refs/heads/ws1"
    );
    let base_commit = git(&["-C", path(&bare), "rev-parse", "HEAD"]);
    let hook_log = fs::read_to_string(f.hook_log()).unwrap();
    assert!(
        hook_log.contains(&format!("ws1/src {} {} 1", "0".repeat(40), base_commit)),
        "post-checkout should run in the final path with worktree-add args: {hook_log}"
    );
    let current = fs::read_to_string(f.root().join("layers/src/current")).unwrap();
    assert_eq!(current.trim(), base_commit);

    // -- Upstream moves; the next workspace fetches it and the base rolls forward.
    fs::write(f.src().join("new.txt"), "new\n").unwrap();
    commit(&f.src(), "second");
    let new_commit = git(&["-C", path(&f.src()), "rev-parse", "HEAD"]);
    f.add_layer("ws2");
    let ws2 = f.repo_dir("ws2");
    assert!(is_mount(&ws2));
    assert_eq!(git(&["-C", path(&ws2), "rev-parse", "HEAD"]), new_commit);
    assert!(ws2.join("new.txt").exists());
    assert_eq!(git(&["-C", path(&ws2), "status", "--porcelain"]), "");
    let current = fs::read_to_string(f.root().join("layers/src/current")).unwrap();
    assert_eq!(current.trim(), new_commit, "base should roll forward");
    let bases: Vec<_> = fs::read_dir(f.root().join("layers/src/bases"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        bases.iter().all(|b| b.starts_with(&new_commit)),
        "old bases should be collected: {bases:?}"
    );

    // -- Isolation: writes in one workspace are invisible to the other.
    fs::write(ws2.join("README.md"), "changed in ws2\n").unwrap();
    assert_eq!(
        fs::read_to_string(ws1.join("README.md")).unwrap(),
        "hello\n"
    );
    assert!(!ws1.join("new.txt").exists());

    // -- Detached image: prune keeps the locked admin dir; switch re-attaches.
    let st = Command::new("hdiutil")
        .args(["detach", "-quiet", path(&ws1)])
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!is_mount(&ws1));
    git(&["-C", path(&bare), "worktree", "prune"]);
    assert!(bare.join("worktrees/box-layer-ws-ws1").exists());
    let out = f.box_cmd(&["workspace", "switch", "ws1"]);
    assert!(out.status.success(), "{out:?}");
    assert!(is_mount(&ws1));
    assert_eq!(git(&["-C", path(&ws1), "status", "--porcelain"]), "");

    // -- Removal detaches and deletes images, admin dirs, and branches.
    let out = f.box_cmd(&["workspace", "rm", "ws1", "ws2"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!f.root().join("workspaces/ws1").exists());
    assert!(!f.root().join("workspaces/ws2").exists());
    assert!(!bare.join("worktrees/box-layer-ws-ws1").exists());
    assert!(!bare.join("worktrees/box-layer-ws-ws2").exists());
    assert_eq!(
        git(&["-C", path(&bare), "branch", "--list", "ws1", "ws2"]),
        ""
    );

    // -- Removing the source drops its base images.
    let out = f.box_cmd(&["source", "remove", "src"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!f.root().join("layers/src").exists());
}

#[test]
fn layer_repo_add_and_remove_in_existing_workspace() {
    let f = Fixture::new();
    f.add_layer("ws");
    let other = f.temp.path().join("other");
    git(&["-c", "init.defaultBranch=main", "init", "-q", path(&other)]);
    fs::write(other.join("x.txt"), "x\n").unwrap();
    commit(&other, "init");
    assert!(f.box_cmd(&["source", "add", path(&other)]).status.success());

    let out = f.box_cmd(&["repo", "add", "other", "--workspace", "ws"]);
    assert!(out.status.success(), "{out:?}");
    let other_dir = f.root().join("workspaces/ws/other");
    assert!(is_mount(&other_dir));
    assert!(other_dir.join("x.txt").exists());

    let out = f.box_cmd(&["repo", "remove", "other", "--workspace", "ws"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!other_dir.exists());
    assert!(!f
        .root()
        .join("repos/other.git/worktrees/box-layer-ws-ws")
        .exists());

    let out = f.box_cmd(&["workspace", "rm", "ws"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!f.root().join("workspaces/ws").exists());
}
