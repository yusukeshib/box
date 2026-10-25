//! Last-activity detection for `box workspace prune`.
//!
//! A workspace counts as used when any of these changed:
//! - files a user or agent edited: tracked files with changes and untracked
//!   (non-ignored) files reported by `git status`, or the nearest existing
//!   parent directory of a deleted path;
//! - git state: `HEAD`, `index`, and `logs/HEAD` in the repo's git dir, which
//!   change on commits, checkouts, resets, rebases, and staging;
//! - for `layer` workspaces, the disk image file, which changes whenever the
//!   mounted volume is written (the image may be detached, so it is not
//!   inspected with git).
//!
//! The probe is read-only: `git status` runs with `--no-optional-locks` so it
//! never refreshes the index and therefore never bumps its own signal.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::config;
use crate::layer;
use crate::repo;
use crate::workspace::Strategy;

/// Files in a git dir whose mtime changes on commits, checkouts, resets,
/// rebases, and staging.
const GIT_DIR_MARKERS: &[&str] = &["HEAD", "index", "logs/HEAD"];

/// Running maximum of observed modification times.
#[derive(Default)]
struct Latest(Option<DateTime<Utc>>);

impl Latest {
    fn observe(&mut self, time: SystemTime) {
        let time = DateTime::<Utc>::from(time);
        if self.0.is_none_or(|current| time > current) {
            self.0 = Some(time);
        }
    }

    /// Observe `path`'s mtime (without following symlinks). Missing paths are
    /// ignored.
    fn observe_path(&mut self, path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                let modified = meta
                    .modified()
                    .with_context(|| format!("Failed to read mtime of {}", path.display()))?;
                self.observe(modified);
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("Failed to stat {}", path.display())),
        }
    }

    fn is_after(&self, cutoff: DateTime<Utc>) -> bool {
        self.0.is_some_and(|t| t > cutoff)
    }
}

/// Latest activity observed in workspace `name`.
///
/// Stops probing as soon as activity newer than `cutoff` is found, so the
/// result is exact only when it is at or before `cutoff`; otherwise it is some
/// activity time after `cutoff`. Returns `None` if nothing could be observed
/// (for example, every repo directory is gone).
pub fn latest_activity(
    name: &str,
    strategy: Strategy,
    repos: &[String],
    cutoff: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    let ws_root = config::box_root()?.join("workspaces").join(name);
    let registry = if strategy == Strategy::Layer {
        repo::list()?
    } else {
        Vec::new()
    };
    let mut latest = Latest::default();
    for repo_name in repos {
        if strategy == Strategy::Layer {
            latest.observe_path(&layer::image_path(&ws_root, repo_name))?;
            if let Some(entry) = registry.iter().find(|r| r.name == *repo_name) {
                observe_git_dir(&mut latest, &layer::ws_admin_dir(&entry.path, name))?;
            }
        } else {
            observe_checkout(&mut latest, &ws_root.join(repo_name), cutoff)?;
        }
        if latest.is_after(cutoff) {
            break;
        }
    }
    Ok(latest.0)
}

fn observe_git_dir(latest: &mut Latest, git_dir: &Path) -> Result<()> {
    for marker in GIT_DIR_MARKERS {
        latest.observe_path(&git_dir.join(marker))?;
    }
    Ok(())
}

/// Observe a clone or worktree checkout at `dir`. A missing directory has no
/// activity (and nothing left to lose).
fn observe_checkout(latest: &mut Latest, dir: &Path, cutoff: DateTime<Utc>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let git_dir = git_stdout(dir, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = git_dir.strip_suffix(b"\n").unwrap_or(&git_dir);
    observe_git_dir(latest, &bytes_to_path(git_dir))?;
    if latest.is_after(cutoff) {
        return Ok(());
    }

    // Submodules are separate checkouts: changes and commits inside them are
    // invisible to the parent's status and git dir, so probe each initialized
    // one recursively. Uninitialized ones have no `.git` and nothing to lose.
    let stage = git_stdout(dir, &["ls-files", "-z", "--stage"])?;
    for sub in submodule_paths(&stage) {
        let sub_dir = dir.join(sub);
        if fs::symlink_metadata(sub_dir.join(".git")).is_ok() {
            observe_checkout(latest, &sub_dir, cutoff)?;
            if latest.is_after(cutoff) {
                return Ok(());
            }
        }
    }

    let status = git_stdout(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?;
    for path in status_paths(&status) {
        observe_existing_ancestor(latest, dir, &dir.join(path))?;
        if latest.is_after(cutoff) {
            break;
        }
    }
    Ok(())
}

/// Observe `path`, or for a deleted path the nearest existing ancestor within
/// `root` (deleting an entry updates its parent directory's mtime).
fn observe_existing_ancestor(latest: &mut Latest, root: &Path, path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(p) = current {
        if latest.observe_path(p)? || p == root {
            break;
        }
        current = p.parent().filter(|parent| parent.starts_with(root));
    }
    Ok(())
}

/// Git prints paths as raw bytes; keep them exact so non-UTF-8 names are
/// statted correctly.
fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// Submodule (gitlink, mode 160000) paths from `git ls-files -z --stage`.
fn submodule_paths(output: &[u8]) -> Vec<PathBuf> {
    output
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let tab = entry.iter().position(|b| *b == b'\t')?;
            let (meta, path) = (&entry[..tab], &entry[tab + 1..]);
            meta.starts_with(b"160000 ").then(|| bytes_to_path(path))
        })
        .collect()
}

/// Paths from `git status --porcelain=v1 -z` output, relative to the repo
/// root. For renames and copies only the new path is returned.
fn status_paths(output: &[u8]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut fields = output.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        paths.push(bytes_to_path(path));
        if matches!(code[0], b'R' | b'C') {
            fields.next();
        }
    }
    paths
}

fn git_stdout(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run git in {}", dir.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        fs::File::open(path).unwrap().set_modified(time).unwrap();
    }

    /// Set the mtime of every entry under `dir` (including git internals) to
    /// `time`, so only later changes look recent.
    fn age_tree(dir: &Path, time: SystemTime) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                age_tree(&path, time);
            }
            set_mtime(&path, time);
        }
        set_mtime(dir, time);
    }

    /// A committed repo whose every mtime is two days old.
    fn old_repo() -> (tempfile::TempDir, std::path::PathBuf, SystemTime) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q"]);
        fs::write(dir.join("src/a.txt"), "a").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-qm", "init"]);
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
        age_tree(&dir, old);
        (tmp, dir, old)
    }

    fn activity(dir: &Path) -> Option<DateTime<Utc>> {
        let mut latest = Latest::default();
        let cutoff = Utc::now() - chrono::Duration::days(1);
        observe_checkout(&mut latest, dir, cutoff).unwrap();
        latest.0
    }

    fn is_recent(t: Option<DateTime<Utc>>) -> bool {
        t.unwrap() > Utc::now() - chrono::Duration::hours(1)
    }

    #[test]
    fn test_untouched_repo_reports_old_activity() {
        let (_tmp, dir, old) = old_repo();
        assert_eq!(activity(&dir).unwrap(), DateTime::<Utc>::from(old));
    }

    #[test]
    fn test_probe_does_not_bump_index() {
        let (_tmp, dir, _) = old_repo();
        // Make the index stat-stale so a plain `git status` would rewrite it.
        fs::write(dir.join("b.txt"), "b").unwrap();
        set_mtime(
            &dir.join("b.txt"),
            SystemTime::now() - Duration::from_secs(3600 * 30),
        );
        let before = fs::metadata(dir.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap();
        activity(&dir);
        let after = fs::metadata(dir.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn test_modified_tracked_file_is_activity() {
        let (_tmp, dir, _) = old_repo();
        fs::write(dir.join("src/a.txt"), "changed").unwrap();
        assert!(is_recent(activity(&dir)));
    }

    #[test]
    fn test_untracked_file_in_new_directory_is_activity() {
        let (_tmp, dir, old) = old_repo();
        fs::create_dir_all(dir.join("new/deep")).unwrap();
        fs::write(dir.join("new/deep/c.txt"), "c").unwrap();
        // Only the file itself is recent; the directories look old.
        set_mtime(&dir.join("new/deep"), old);
        set_mtime(&dir.join("new"), old);
        set_mtime(&dir, old);
        assert!(is_recent(activity(&dir)));
    }

    #[test]
    fn test_ignored_file_is_not_activity() {
        let (_tmp, dir, old) = old_repo();
        fs::create_dir_all(dir.join("ignored")).unwrap();
        fs::write(dir.join("ignored/x"), "x").unwrap();
        set_mtime(&dir.join("ignored"), old);
        set_mtime(&dir, old);
        assert_eq!(activity(&dir).unwrap(), DateTime::<Utc>::from(old));
    }

    #[test]
    fn test_deleted_file_is_activity() {
        let (_tmp, dir, _) = old_repo();
        fs::remove_file(dir.join("src/a.txt")).unwrap();
        assert!(is_recent(activity(&dir)));
    }

    #[test]
    fn test_commit_is_activity() {
        let (_tmp, dir, old) = old_repo();
        fs::write(dir.join("b.txt"), "changed").unwrap();
        git(&dir, &["commit", "-qam", "change"]);
        set_mtime(&dir.join("b.txt"), old);
        assert!(is_recent(activity(&dir)));
    }

    #[test]
    fn test_missing_checkout_has_no_activity() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(activity(&tmp.path().join("gone")).is_none());
    }

    #[test]
    fn test_status_paths_parses_renames_and_spaces() {
        let out = b" M src/a b.txt\0R  new.txt\0old.txt\0?? x/y.txt\0 D gone.txt\0";
        let expected: Vec<PathBuf> = ["src/a b.txt", "new.txt", "x/y.txt", "gone.txt"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(status_paths(out), expected);
    }

    #[test]
    fn test_status_paths_keeps_non_utf8_bytes() {
        let out = b"?? caf\xe9.txt\0";
        assert_eq!(status_paths(out), vec![bytes_to_path(b"caf\xe9.txt")]);
    }

    #[test]
    fn test_non_utf8_untracked_file_is_activity() {
        let (_tmp, dir, old) = old_repo();
        let path = dir.join(bytes_to_path(b"caf\xe9.txt"));
        if fs::write(&path, "x").is_err() {
            return; // filesystem rejects non-UTF-8 names (e.g. APFS)
        }
        set_mtime(&dir, old);
        assert!(is_recent(activity(&dir)));
    }

    #[test]
    fn test_submodule_paths_selects_gitlinks() {
        let out = b"100644 aaaa 0\tsrc/a.txt\x00160000 bbbb 0\tlib/sub\x00";
        assert_eq!(submodule_paths(out), vec![PathBuf::from("lib/sub")]);
    }

    #[test]
    fn test_submodule_edit_is_activity() {
        let (_tmp, dir, old) = old_repo();
        let (_sub_tmp, sub_src, _) = old_repo();
        git(
            &dir,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                sub_src.to_str().unwrap(),
                "lib/sub",
            ],
        );
        git(&dir, &["commit", "-qm", "add submodule"]);
        age_tree(&dir, old);
        assert_eq!(activity(&dir).unwrap(), DateTime::<Utc>::from(old));

        fs::write(dir.join("lib/sub/src/a.txt"), "changed").unwrap();
        assert!(is_recent(activity(&dir)));
    }
}
