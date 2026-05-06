use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config;

#[derive(Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub path: String,
}

pub fn repos_dir() -> Result<PathBuf> {
    Ok(config::box_root()?.join("repos"))
}

/// Migrate the old flat-file `~/.box/repos` registry to bare clones under
/// `~/.box/repos/`. Each line in the old file is a path to a git repo;
/// we bare-clone it into `~/.box/repos/<name>.git`.
fn migrate_old_repos_file() -> Result<()> {
    let path = config::box_root()?.join("repos");
    if !path.is_file() {
        return Ok(());
    }
    let content = fs::read_to_string(&path)?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        fs::remove_file(&path)?;
        return Ok(());
    }

    // Rename the old file so we can create the directory.
    // Use a timestamped backup name to avoid collisions.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = config::box_root()?.join(format!("repos.{}.bak", ts));
    fs::rename(&path, &backup)?;
    let dir = config::box_root()?.join("repos");
    fs::create_dir_all(&dir)?;

    eprintln!("\x1b[2mMigrating repos to bare clones…\x1b[0m");
    let mut had_failures = false;
    for line in &lines {
        let repo_path = line.trim();
        let name = Path::new(repo_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let dest = dir.join(format!("{}.git", name));
        if dest.exists() {
            continue;
        }
        eprintln!("  \x1b[1m{}\x1b[0m", name);
        let dest_str = dest.to_string_lossy().to_string();
        let status = Command::new("git")
            .args(["clone", "--bare", repo_path, &dest_str])
            .status();
        match status {
            Ok(s) if s.success() => {
                if let Err(e) = configure_fetch_refspec(&dest_str) {
                    eprintln!("    \x1b[31mfailed to configure refspec: {}\x1b[0m", e);
                    had_failures = true;
                }
                repoint_origin(repo_path, &dest_str);
            }
            _ => {
                eprintln!("    \x1b[31mfailed to bare-clone, skipping\x1b[0m");
                had_failures = true;
            }
        }
    }

    if had_failures {
        eprintln!(
            "\x1b[33mSome repos failed to migrate. Old registry kept at: {}\x1b[0m",
            backup.display()
        );
    } else {
        let _ = fs::remove_file(&backup);
    }
    eprintln!();
    Ok(())
}

pub fn list() -> Result<Vec<RepoEntry>> {
    let dir = repos_dir()?;

    // Auto-migrate old flat-file format
    if dir.is_file() {
        migrate_old_repos_file()?;
    }

    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if let Some(name) = dir_name.strip_suffix(".git") {
            if !name.is_empty() && path.join("HEAD").exists() {
                let path_str = path.to_string_lossy().to_string();
                // Ensure fetch refspec is set (repairs bare repos created before this fix)
                ensure_fetch_refspec(&path_str);
                // Fix box/* session branches that inherited main's upstream
                repair_box_branch_upstreams(&path_str);
                entries.push(RepoEntry {
                    name: name.to_string(),
                    path: path_str,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

pub fn add(path: &str) -> Result<()> {
    let canonical =
        fs::canonicalize(path).map_err(|_| anyhow::anyhow!("Path '{}' does not exist.", path))?;
    let canonical_str = canonical.to_string_lossy().to_string();

    if !crate::git::is_repo(&canonical) {
        bail!("'{}' is not a git repository.", canonical_str);
    }

    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("Cannot derive repo name from path."))?;

    let existing = list()?;
    for entry in &existing {
        if entry.name == name {
            bail!("A repo named '{}' is already registered.", name);
        }
    }

    let dir = repos_dir()?;
    fs::create_dir_all(&dir)?;

    let dest = dir.join(format!("{}.git", name));
    let dest_str = dest.to_string_lossy().to_string();

    eprintln!("\x1b[2mbare-cloning {}…\x1b[0m", name);
    let status = Command::new("git")
        .args(["clone", "--bare", &canonical_str, &dest_str])
        .status()?;
    if !status.success() {
        bail!("git clone --bare failed for '{}'.", name);
    }

    // git clone --bare doesn't set a fetch refspec, so git fetch won't
    // update local branches. Configure it so fetch maps remote branches
    // directly onto the bare repo's local refs.
    configure_fetch_refspec(&dest_str)?;

    // Repoint origin to the actual remote URL (not the local path)
    repoint_origin(&canonical_str, &dest_str);

    eprintln!("Registered repo '\x1b[1m{}\x1b[0m' (bare clone)", name);
    Ok(())
}

/// Check and fix fetch refspec and `push.autoSetupRemote` on an existing bare
/// repo. Repairs bares that are missing a refspec entirely (clone --bare
/// default), bares that only have the legacy single +refs/heads/*:refs/heads/*
/// line (pre-origin-mapping versions of box), and bares created before
/// `push.autoSetupRemote` was added.
fn ensure_fetch_refspec(bare_dir: &str) {
    let needs_refspec = match Command::new("git")
        .args(["-C", bare_dir, "config", "--get-all", "remote.origin.fetch"])
        .output()
    {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            !text.lines().any(|l| l.trim() == REFSPEC_ORIGIN)
        }
        _ => true,
    };
    let needs_autosetup = match Command::new("git")
        .args(["-C", bare_dir, "config", "--get", "push.autoSetupRemote"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() != "true",
        _ => true,
    };
    if needs_refspec || needs_autosetup {
        let _ = configure_fetch_refspec(bare_dir);
    }
}

/// Repair `branch.box/<session>.merge` entries that point at the start-point's
/// upstream (typically `refs/heads/main`) instead of the session branch's own
/// ref. Stock `git worktree add -b box/<session>` from a bare carries main's
/// tracking forward, which silently breaks `git push` (default `simple` mode
/// refuses on name mismatch) and `git push --force-with-lease` (the lease
/// check then targets `origin/main`'s SHA). New sessions get this right via
/// `workspace::set_self_upstream`; this is the migration for sessions created
/// before that fix.
fn repair_box_branch_upstreams(bare_dir: &str) {
    let output = Command::new("git")
        .args([
            "-C",
            bare_dir,
            "config",
            "--get-regexp",
            r"^branch\.box/.+\.merge$",
        ])
        .output();
    let Ok(output) = output else { return };
    if !output.status.success() {
        // Exit 1 means no matching keys — nothing to repair.
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let Some(branch) = key
            .strip_prefix("branch.")
            .and_then(|s| s.strip_suffix(".merge"))
        else {
            continue;
        };
        let expected = format!("refs/heads/{}", branch);
        if value == expected {
            continue;
        }
        let _ = Command::new("git")
            .args([
                "-C",
                bare_dir,
                "config",
                &format!("branch.{}.merge", branch),
                &expected,
            ])
            .output();
        let _ = Command::new("git")
            .args([
                "-C",
                bare_dir,
                "config",
                &format!("branch.{}.remote", branch),
                "origin",
            ])
            .output();
    }
}

/// Refspec that mirrors the remote's branches onto the bare repo's local
/// heads. Required so `git clone --local <bare>` and `git worktree add` see
/// up-to-date refs.
const REFSPEC_HEADS: &str = "+refs/heads/*:refs/heads/*";

/// Refspec that also exposes remote branches under the conventional
/// `refs/remotes/origin/*` namespace, so `git rebase origin/main` works inside
/// box workspaces without any special handling.
const REFSPEC_ORIGIN: &str = "+refs/heads/*:refs/remotes/origin/*";

/// `git clone --bare` does not set `remote.origin.fetch`, so `git fetch` will
/// download objects but never update local refs. We configure two refspecs:
/// the heads/heads line preserves the bare-as-cache semantics that `box new`
/// (clone + worktree strategies) relies on, and the heads/remotes line
/// produces the conventional `origin/<branch>` refs that humans and tools
/// expect when they `git fetch && git rebase origin/main`.
///
/// We also set `push.autoSetupRemote = true` so the first `git push` of a
/// branch box created locally records its upstream — without that,
/// `refs/remotes/origin/<branch>` is never written and `git push
/// --force-with-lease` fails with "no such ref".
fn configure_fetch_refspec(bare_dir: &str) -> Result<()> {
    // Clear any pre-existing refspec lines so re-running on a partially
    // configured bare doesn't leave duplicates.
    let _ = Command::new("git")
        .args([
            "-C",
            bare_dir,
            "config",
            "--unset-all",
            "remote.origin.fetch",
        ])
        .status();
    for refspec in [REFSPEC_HEADS, REFSPEC_ORIGIN] {
        let status = Command::new("git")
            .args([
                "-C",
                bare_dir,
                "config",
                "--add",
                "remote.origin.fetch",
                refspec,
            ])
            .status()?;
        if !status.success() {
            bail!("Failed to configure fetch refspec for '{}'.", bare_dir);
        }
    }
    let status = Command::new("git")
        .args(["-C", bare_dir, "config", "push.autoSetupRemote", "true"])
        .status()?;
    if !status.success() {
        bail!("Failed to set push.autoSetupRemote for '{}'.", bare_dir);
    }
    Ok(())
}

/// Repoint the bare clone's origin from the local source path to the source's
/// actual remote URL (e.g. GitHub). If the source has no remote, leave as-is.
fn repoint_origin(source_dir: &str, bare_dir: &str) {
    if let Ok(output) = Command::new("git")
        .args(["-C", source_dir, "remote", "get-url", "origin"])
        .output()
    {
        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !url.is_empty() {
                let _ = Command::new("git")
                    .args(["-C", bare_dir, "remote", "set-url", "origin", &url])
                    .status();
            }
        }
    }
}

/// Get the origin remote URL from a bare clone, for display purposes.
pub fn origin_url(path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", path, "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            Some(url)
        } else {
            None
        }
    } else {
        None
    }
}

pub fn remove(name: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name == ".." || name == "." {
        bail!("Invalid repo name '{}'.", name);
    }
    let dir = repos_dir()?;
    let bare = dir.join(format!("{}.git", name));
    // Ensure the resolved path stays within repos_dir
    let canonical_bare = fs::canonicalize(&bare)
        .map_err(|_| anyhow::anyhow!("No repo named '{}' is registered.", name))?;
    let canonical_dir = fs::canonicalize(&dir)?;
    if !canonical_bare.starts_with(&canonical_dir) {
        bail!("Invalid repo name '{}'.", name);
    }
    fs::remove_dir_all(&canonical_bare)?;
    eprintln!("Removed repo '{}'.", name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ENV_LOCK;

    fn with_temp_home<F: FnOnce(&Path)>(f: F) {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f(tmp.path());
        }));
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    fn make_git_repo(base: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_str().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", dir_str])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git init failed");
        // Create an initial commit so the repo has a HEAD
        let status = std::process::Command::new("git")
            .args([
                "-C",
                dir_str,
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@test.com",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git commit failed");
        dir
    }

    #[test]
    fn test_list_empty() {
        with_temp_home(|_| {
            let repos = list().unwrap();
            assert!(repos.is_empty());
        });
    }

    #[test]
    fn test_add_and_list() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "my-app");
            add(repo.to_str().unwrap()).unwrap();

            let repos = list().unwrap();
            assert_eq!(repos.len(), 1);
            assert_eq!(repos[0].name, "my-app");
            assert!(repos[0].path.ends_with("my-app.git"));
        });
    }

    #[test]
    fn test_add_duplicate_name() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "my-app");
            add(repo.to_str().unwrap()).unwrap();
            let err = add(repo.to_str().unwrap()).unwrap_err();
            assert!(err.to_string().contains("already registered"));
        });
    }

    #[test]
    fn test_add_not_git_repo() {
        with_temp_home(|home| {
            let dir = home.join("not-a-repo");
            fs::create_dir_all(&dir).unwrap();
            let err = add(dir.to_str().unwrap()).unwrap_err();
            assert!(err.to_string().contains("not a git repository"));
        });
    }

    #[test]
    fn test_remove() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "my-app");
            add(repo.to_str().unwrap()).unwrap();
            remove("my-app").unwrap();

            let repos = list().unwrap();
            assert!(repos.is_empty());
        });
    }

    #[test]
    fn test_remove_not_found() {
        with_temp_home(|_| {
            let err = remove("nonexistent").unwrap_err();
            assert!(err.to_string().contains("No repo named"));
        });
    }

    #[test]
    fn test_add_multiple_and_remove_one() {
        with_temp_home(|home| {
            let repo_a = make_git_repo(home, "app-a");
            let repo_b = make_git_repo(home, "app-b");
            add(repo_a.to_str().unwrap()).unwrap();
            add(repo_b.to_str().unwrap()).unwrap();

            remove("app-a").unwrap();
            let repos = list().unwrap();
            assert_eq!(repos.len(), 1);
            assert_eq!(repos[0].name, "app-b");
        });
    }

    #[test]
    fn test_bare_clone_has_head() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "check-head");
            add(repo.to_str().unwrap()).unwrap();

            let repos = list().unwrap();
            assert_eq!(repos.len(), 1);
            let bare_path = Path::new(&repos[0].path);
            assert!(bare_path.join("HEAD").exists());
        });
    }

    #[test]
    fn test_bare_clone_has_fetch_refspec() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "my-app");
            add(repo.to_str().unwrap()).unwrap();

            let repos = list().unwrap();
            let output = Command::new("git")
                .args([
                    "-C",
                    &repos[0].path,
                    "config",
                    "--get-all",
                    "remote.origin.fetch",
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .collect();
            assert_eq!(
                lines,
                vec![
                    "+refs/heads/*:refs/heads/*".to_string(),
                    "+refs/heads/*:refs/remotes/origin/*".to_string(),
                ]
            );

            let auto = Command::new("git")
                .args([
                    "-C",
                    &repos[0].path,
                    "config",
                    "--get",
                    "push.autoSetupRemote",
                ])
                .output()
                .unwrap();
            assert!(auto.status.success());
            assert_eq!(String::from_utf8_lossy(&auto.stdout).trim(), "true");
        });
    }

    #[test]
    fn test_ensure_fetch_refspec_sets_missing_push_autosetup() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "needs-autosetup");
            add(repo.to_str().unwrap()).unwrap();

            // Simulate a bare from a box version that configured the fetch
            // refspec but not push.autoSetupRemote.
            let repos = list().unwrap();
            let bare = &repos[0].path;
            let s = Command::new("git")
                .args(["-C", bare, "config", "--unset", "push.autoSetupRemote"])
                .status()
                .unwrap();
            assert!(s.success());

            let _ = list().unwrap();

            let auto = Command::new("git")
                .args(["-C", bare, "config", "--get", "push.autoSetupRemote"])
                .output()
                .unwrap();
            assert!(auto.status.success());
            assert_eq!(String::from_utf8_lossy(&auto.stdout).trim(), "true");
        });
    }

    #[test]
    fn test_ensure_fetch_refspec_migrates_legacy_single_line() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "legacy");
            add(repo.to_str().unwrap()).unwrap();

            // Simulate a bare cloned by an older box version that only knew
            // about the heads/heads refspec.
            let repos = list().unwrap();
            let bare = &repos[0].path;
            let s = Command::new("git")
                .args(["-C", bare, "config", "--unset-all", "remote.origin.fetch"])
                .status()
                .unwrap();
            assert!(s.success());
            let s = Command::new("git")
                .args([
                    "-C",
                    bare,
                    "config",
                    "--add",
                    "remote.origin.fetch",
                    "+refs/heads/*:refs/heads/*",
                ])
                .status()
                .unwrap();
            assert!(s.success());

            // list() invokes ensure_fetch_refspec on each bare; the second
            // pass should re-add the origin/* line.
            let _ = list().unwrap();

            let output = Command::new("git")
                .args(["-C", bare, "config", "--get-all", "remote.origin.fetch"])
                .output()
                .unwrap();
            let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .collect();
            assert!(lines.contains(&"+refs/heads/*:refs/remotes/origin/*".to_string()));
        });
    }

    #[test]
    fn test_list_repairs_misconfigured_box_branch_upstreams() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "with-sessions");
            add(repo.to_str().unwrap()).unwrap();
            let bare = list().unwrap()[0].path.clone();

            // Simulate two session branches created by an older box version:
            // one misconfigured (tracks main), one configured by hand to track
            // a sibling branch (we leave that one alone — only fix `box/*`
            // entries that point at the wrong place).
            for (branch, merge) in [
                ("box/foo", "refs/heads/main"),
                ("box/bar", "refs/heads/box/bar"), // already correct
                ("feature/baz", "refs/heads/main"), // not a box branch — ignore
            ] {
                let s = Command::new("git")
                    .args([
                        "-C",
                        &bare,
                        "config",
                        &format!("branch.{}.remote", branch),
                        "origin",
                    ])
                    .status()
                    .unwrap();
                assert!(s.success());
                let s = Command::new("git")
                    .args([
                        "-C",
                        &bare,
                        "config",
                        &format!("branch.{}.merge", branch),
                        merge,
                    ])
                    .status()
                    .unwrap();
                assert!(s.success());
            }

            // list() invokes repair_box_branch_upstreams on each bare.
            let _ = list().unwrap();

            let value = |key: &str| -> String {
                let out = Command::new("git")
                    .args(["-C", &bare, "config", "--get", key])
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            };

            assert_eq!(value("branch.box/foo.merge"), "refs/heads/box/foo");
            assert_eq!(value("branch.box/bar.merge"), "refs/heads/box/bar");
            // feature/* untouched — only box/* sessions are box's responsibility.
            assert_eq!(value("branch.feature/baz.merge"), "refs/heads/main");
        });
    }

    #[test]
    fn test_fetch_updates_bare_branches() {
        with_temp_home(|home| {
            // Create a source repo with a remote (simulated via a bare intermediary)
            let remote_dir = home.join("remote.git");
            let remote_str = remote_dir.to_str().unwrap();
            let s = Command::new("git")
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "init",
                    "--bare",
                    remote_str,
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git init --bare failed");

            let source = home.join("source");
            let source_str = source.to_str().unwrap();
            let s = Command::new("git")
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "clone",
                    remote_str,
                    source_str,
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git clone failed");
            let s = Command::new("git")
                .args([
                    "-C",
                    source_str,
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@test.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "commit 1",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git commit 1 failed");
            let s = Command::new("git")
                .args(["-C", source_str, "push", "origin", "main"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git push 1 failed");

            // Register the source repo (creates bare clone of source,
            // then repoints origin to remote.git)
            add(source_str).unwrap();
            let repos = list().unwrap();
            let bare_path = &repos[0].path;

            // `add()` already configured the bare clone's origin to match
            // the source repo's origin (remote.git); record the pre-fetch log.
            let log_before = Command::new("git")
                .args(["-C", bare_path, "log", "--oneline", "main"])
                .output()
                .unwrap();
            let count_before = String::from_utf8_lossy(&log_before.stdout).lines().count();

            // Push a new commit via source
            let s = Command::new("git")
                .args([
                    "-C",
                    source_str,
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@test.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "commit 2",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git commit 2 failed");
            let s = Command::new("git")
                .args(["-C", source_str, "push", "origin", "main"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git push 2 failed");

            // Fetch on bare repo
            let s = Command::new("git")
                .args(["-C", bare_path, "fetch", "--all"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(s.success(), "git fetch failed");

            let log_after = Command::new("git")
                .args(["-C", bare_path, "log", "--oneline", "main"])
                .output()
                .unwrap();
            let count_after = String::from_utf8_lossy(&log_after.stdout).lines().count();

            assert_eq!(count_before, 1);
            assert_eq!(
                count_after, 2,
                "fetch should update the bare repo's main branch"
            );
        });
    }
}
