use anyhow::{bail, Result};
use std::fmt;
use std::process::Command;

use crate::config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    Clone,
    Worktree,
}

impl Strategy {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "clone" => Ok(Strategy::Clone),
            "worktree" => Ok(Strategy::Worktree),
            other => bail!("Unknown strategy '{}'. Use 'clone' or 'worktree'.", other),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Strategy::Clone => "clone",
            Strategy::Worktree => "worktree",
        }
    }
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Remove multiple sessions' workspaces in a single unified progress bar.
///
/// Every repo across every session becomes one unit in the same bar — matching
/// the unified experience of `ensure_workspace_multi*`. Each unit is a plain
/// `fs::remove_dir_all` of the worktree/clone directory (much faster than
/// `git worktree remove --force`, which scans the worktree). After the bar
/// finishes we run `git worktree prune` + a single batched `git branch -D`
/// per bare repo to clear leftover admin entries and session branches, then
/// remove the now-empty workspace root dir for each session.
pub fn remove_sessions(
    sessions: &[(String, Strategy, Vec<String>)],
    verbose: bool,
) -> Result<std::collections::BTreeSet<String>> {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = config::box_root()?;
    let mut failed_sessions = std::collections::BTreeSet::new();

    // Each work item is just a directory to delete on disk; the actual git
    // bookkeeping (worktree admin entries, branches) is batched per-bare-repo
    // after the parallel phase so we don't pay 2 git subprocesses per repo.
    let mut items: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut all_worktree = true;

    // Branches to delete per bare-repo path: same session name across multiple
    // repos becomes one git invocation per bare repo.
    let mut per_bare_branches: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    // Bare paths that need `git worktree prune` after we delete worktree dirs
    // via fs::remove_dir_all (git's admin entries linger otherwise).
    let mut bares_to_prune: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for (name, strategy, repo_names) in sessions {
        if matches!(strategy, Strategy::Clone) {
            all_worktree = false;
        }
        let branch = name.clone();
        for repo_name in repo_names {
            let dest = root.join("workspaces").join(name).join(repo_name);
            items.push((format!("{}/{}", name, repo_name), dest));

            if matches!(strategy, Strategy::Worktree) {
                if let Some(repo) = all_repos.iter().find(|r| r.name == *repo_name) {
                    bares_to_prune.insert(repo.path.clone());
                    per_bare_branches
                        .entry(repo.path.clone())
                        .or_default()
                        .push(branch.clone());
                }
            }
        }
    }

    if !items.is_empty() {
        let count = items.len();
        let noun = if all_worktree { "worktree" } else { "repo" };
        let label = format!(
            "Removing {} {}{}",
            count,
            noun,
            if count == 1 { "" } else { "s" }
        );

        let results = crate::progress::run_parallel_with_progress(
            &label,
            items,
            verbose,
            false,
            |_name, dest| match std::fs::remove_dir_all(&dest) {
                Ok(()) => (true, String::new()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (true, String::new()),
                Err(e) => (
                    false,
                    format!("failed to remove '{}': {}", dest.display(), e),
                ),
            },
        );

        if verbose {
            for result in &results {
                eprintln!("\x1b[2mremove {}:\x1b[0m", result.name);
                if !result.output.is_empty() {
                    eprint!("{}", result.output);
                }
                if result.success {
                    eprintln!("  \x1b[32mok\x1b[0m");
                } else {
                    eprintln!("  \x1b[31mfailed\x1b[0m");
                }
            }
        } else {
            for f in results.iter().filter(|r| !r.success) {
                eprintln!("  \x1b[1m{}\x1b[0m: {}", f.name, f.output.trim());
            }
        }
        for result in results.iter().filter(|result| !result.success) {
            if let Some((session, _)) = result.name.split_once('/') {
                failed_sessions.insert(session.to_string());
            }
        }
    }

    // Per-bare cleanup: prune stale worktree admin entries + delete the
    // session branches in a single `git branch -D` per bare. This replaces
    // the previous per-repo 2-subprocess pattern (worktree remove + branch -D).
    // Parallelize across bares.
    if !bares_to_prune.is_empty() {
        let cleanup_items: Vec<(String, (String, Vec<String>))> = bares_to_prune
            .into_iter()
            .map(|bare| {
                let branches = per_bare_branches.remove(&bare).unwrap_or_default();
                (bare.clone(), (bare, branches))
            })
            .collect();

        // Cleanup tasks intentionally don't propagate `git branch -D` failures
        // into `success`: a session being removed when its branch is already
        // gone is a normal race (e.g. half-cleaned-up state, manual `git
        // branch -D`, repo unregistered then re-registered). We do log the
        // git output into `buf` either way so verbose mode shows what happened.
        let cleanup_results =
            crate::parallel::run_parallel(cleanup_items, |_name, (bare, branches)| {
                let mut buf = String::new();
                let mut success = true;
                if let Err(e) = run_git_capture(&["-C", &bare, "worktree", "prune"], &mut buf) {
                    success = false;
                    buf.push_str(&format!("failed to run git worktree prune: {}\n", e));
                }
                // Dedupe just in case the same branch landed in the list twice.
                let mut uniq: Vec<&str> = branches.iter().map(|s| s.as_str()).collect();
                uniq.sort_unstable();
                uniq.dedup();
                if !uniq.is_empty() {
                    let mut args: Vec<&str> = vec!["-C", &bare, "branch", "-D"];
                    args.extend(uniq.iter().copied());
                    if let Err(e) = run_git_capture(&args, &mut buf) {
                        buf.push_str(&format!("git branch -D: {}\n", e));
                    }
                }
                (success, buf)
            });

        if verbose {
            for r in &cleanup_results {
                if !r.output.is_empty() {
                    eprintln!("\x1b[2mcleanup {}:\x1b[0m", r.name);
                    eprint!("{}", r.output);
                }
            }
        }
    }

    for (name, _, _) in sessions {
        let session_root = root.join("workspaces").join(name);
        match std::fs::remove_dir_all(&session_root) {
            Ok(()) => {
                failed_sessions.remove(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                failed_sessions.remove(name);
            }
            Err(e) => {
                eprintln!("Failed to remove '{}': {}", session_root.display(), e);
                failed_sessions.insert(name.clone());
            }
        }
    }

    Ok(failed_sessions)
}

/// Run a git command, append its combined stdout+stderr to `buf`, and return
/// Err if git returned non-zero or the process couldn't be spawned. Used by
/// per-bare worktree-prune + branch-D batching where we want a single error
/// path covering both spawn failure and exit-status failure.
fn run_git_capture(args: &[&str], buf: &mut String) -> std::io::Result<()> {
    let output = Command::new("git").args(args).output()?;
    buf.push_str(&captured_output(&output));
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git exited {}",
            output.status
        )));
    }
    Ok(())
}

/// Build the empty workspace root for a session and ensure its permissions.
fn prepare_workspace_root(session_name: &str) -> Result<std::path::PathBuf> {
    let root = config::box_root()?.join("workspaces").join(session_name);
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&root)?.permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&root, perms)?;
    }
    Ok(root)
}

/// Filter `repos` to those that are not already set up at `root/<name>` and
/// pair each one with its destination path string.
fn pending_repos(
    root: &std::path::Path,
    repos: &[crate::repo::RepoEntry],
) -> Vec<(String, (crate::repo::RepoEntry, String))> {
    repos
        .iter()
        .filter(|repo| !root.join(&repo.name).join(".git").exists())
        .map(|repo| {
            let dest_str = root.join(&repo.name).to_string_lossy().to_string();
            (repo.name.clone(), (repo.clone(), dest_str))
        })
        .collect()
}

/// Format the progress-bar label: `Preparing N repos` when fetching, otherwise
/// `<noun> N <unit>s`.
fn progress_label(noun: &str, unit: &str, count: usize, fetch: bool) -> String {
    let plural = if count == 1 { "" } else { "s" };
    if fetch {
        format!("Preparing {} repo{}", count, plural)
    } else {
        format!("{} {} {}{}", noun, count, unit, plural)
    }
}

/// Print per-task output (verbose) and bail with a combined message if any task
/// failed. `action` is the human-readable verb that shows up in error messages
/// (e.g. "git clone --local"); `verbose_prefix` is the line printed before each
/// task's captured log in verbose mode (e.g. "cloning").
fn report_results(
    results: &[crate::parallel::TaskResult],
    verbose: bool,
    action: &str,
    verbose_prefix: &str,
) -> Result<()> {
    let mut failure_msgs = Vec::new();
    if verbose {
        for result in results {
            eprintln!("\x1b[2m{} {}:\x1b[0m", verbose_prefix, result.name);
            if !result.output.is_empty() {
                eprint!("{}", result.output);
            }
            if !result.success {
                failure_msgs.push(result.name.clone());
            }
        }
    } else {
        for result in results {
            if !result.success {
                failure_msgs.push(format!("  {}: {}", result.name, result.output.trim()));
            }
        }
    }
    if !failure_msgs.is_empty() {
        if verbose {
            bail!("{} failed for: {}", action, failure_msgs.join(", "));
        } else {
            bail!("{} failed:\n{}", action, failure_msgs.join("\n"));
        }
    }
    Ok(())
}

/// Create a multi-repo workspace using git clone --local.
///
/// When `fetch` is true, each repo is fetched from `origin` before cloning so
/// the session picks up the latest upstream refs; fetch failures are
/// captured but do not abort the clone.
pub fn ensure_workspace_multi(
    session_name: &str,
    repos: &[crate::repo::RepoEntry],
    fetch: bool,
    verbose: bool,
) -> Result<String> {
    let root = prepare_workspace_root(session_name)?;
    let to_clone = pending_repos(&root, repos);

    if !to_clone.is_empty() {
        let root_str = root.to_string_lossy().to_string();
        let label = progress_label("Cloning", "repo", to_clone.len(), fetch);
        let results = crate::progress::run_parallel_with_progress(
            &label,
            to_clone,
            verbose,
            true,
            move |_name, (repo, dest_str)| {
                let mut buf = String::new();
                if fetch {
                    let (ok, fetch_log) = crate::git::fetch_repo(&repo);
                    buf.push_str(&fetch_log);
                    if !ok {
                        buf.push_str("  \x1b[33mfetch failed; cloning local refs\x1b[0m\n");
                    }
                }

                let result = Command::new("git")
                    .args(["clone", "--local", &repo.path, &dest_str])
                    .current_dir(&root_str)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .output();
                match result {
                    Ok(output) => {
                        buf.push_str(&captured_output(&output));
                        if output.status.success() {
                            crate::repo::repoint_origin(&repo.path, &dest_str);
                            (true, buf)
                        } else {
                            buf.push_str(&format!(
                                "git clone --local failed for '{}'\n",
                                repo.name
                            ));
                            (false, buf)
                        }
                    }
                    Err(e) => {
                        buf.push_str(&format!("failed to run git: {}\n", e));
                        (false, buf)
                    }
                }
            },
        );

        report_results(&results, verbose, "git clone --local", "cloning")?;
    }

    Ok(root.to_string_lossy().to_string())
}

/// Create a multi-repo workspace using git worktree add.
///
/// When `fetch` is true, each source repo is fetched from `origin` before the
/// worktree is created so the new branch starts from the latest upstream;
/// fetch failures are captured but do not abort the worktree creation.
pub fn ensure_workspace_multi_worktree(
    session_name: &str,
    repos: &[crate::repo::RepoEntry],
    fetch: bool,
    verbose: bool,
) -> Result<String> {
    let root = prepare_workspace_root(session_name)?;
    let branch_name = session_name.to_string();
    let to_create: Vec<(String, (crate::repo::RepoEntry, String, String))> =
        pending_repos(&root, repos)
            .into_iter()
            .map(|(name, (repo, dest))| (name, (repo, dest, branch_name.clone())))
            .collect();

    if !to_create.is_empty() {
        let label = progress_label("Creating", "worktree", to_create.len(), fetch);
        let results = crate::progress::run_parallel_with_progress(
            &label,
            to_create,
            verbose,
            true,
            move |_name, (repo, dest_str, branch)| {
                let mut buf = String::new();
                if fetch {
                    let (ok, fetch_log) = crate::git::fetch_repo(&repo);
                    buf.push_str(&fetch_log);
                    if !ok {
                        buf.push_str("  \x1b[33mfetch failed; branching from local HEAD\x1b[0m\n");
                    }
                }

                // Try with -b first to create a new branch
                let result = Command::new("git")
                    .args([
                        "-C", &repo.path, "worktree", "add", &dest_str, "-b", &branch,
                    ])
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .output();

                let success = match result {
                    Ok(output) if output.status.success() => {
                        buf.push_str(&captured_output(&output));
                        true
                    }
                    Ok(first_output) => {
                        buf.push_str(&captured_output(&first_output));
                        // Branch may already exist from a partial retry; try without -b
                        match Command::new("git")
                            .args(["-C", &repo.path, "worktree", "add", &dest_str, &branch])
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .output()
                        {
                            Ok(output2) => {
                                buf.push_str(&captured_output(&output2));
                                output2.status.success()
                            }
                            Err(e) => {
                                buf.push_str(&format!("failed to run git: {}\n", e));
                                false
                            }
                        }
                    }
                    Err(e) => {
                        buf.push_str(&format!("failed to run git: {}\n", e));
                        false
                    }
                };

                if success {
                    set_self_upstream(&dest_str, &branch);
                }
                (success, buf)
            },
        );

        report_results(&results, verbose, "git worktree add", "worktree")?;
    }

    Ok(root.to_string_lossy().to_string())
}

/// Remove a single repo subdirectory from a session workspace (clone strategy).
pub fn remove_repo_from_workspace(session_name: &str, repo_name: &str) {
    if let Ok(root) = config::box_root() {
        let dir = root.join("workspaces").join(session_name).join(repo_name);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Remove a single repo worktree from a session.
pub fn remove_repo_from_workspace_worktree(session_name: &str, repo_name: &str) {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = match config::box_root() {
        Ok(r) => r,
        Err(_) => return,
    };
    let branch_name = session_name.to_string();
    let dest = root.join("workspaces").join(session_name).join(repo_name);

    // Skip `git worktree remove --force` (slow: it scans the worktree).
    // Delete the files directly, then have git clean up its admin entry and
    // the session branch. Same pattern as `remove_sessions`.
    let _ = std::fs::remove_dir_all(&dest);

    if let Some(entry) = all_repos.iter().find(|r| r.name == repo_name) {
        let _ = Command::new("git")
            .args(["-C", &entry.path, "worktree", "prune"])
            .status();
        let _ = Command::new("git")
            .args(["-C", &entry.path, "branch", "-D", &branch_name])
            .status();
    }
}

// -- Dispatch wrappers --

/// Create workspace using the given strategy.
pub fn ensure_workspace(
    name: &str,
    repos: &[crate::repo::RepoEntry],
    strategy: Strategy,
    fetch: bool,
    verbose: bool,
) -> Result<String> {
    match strategy {
        Strategy::Clone => ensure_workspace_multi(name, repos, fetch, verbose),
        Strategy::Worktree => ensure_workspace_multi_worktree(name, repos, fetch, verbose),
    }
}

/// Remove a single repo from workspace using the given strategy.
pub fn remove_repo_by_strategy(session_name: &str, repo_name: &str, strategy: Strategy) {
    match strategy {
        Strategy::Clone => remove_repo_from_workspace(session_name, repo_name),
        Strategy::Worktree => remove_repo_from_workspace_worktree(session_name, repo_name),
    }
}

/// Combine stdout and stderr from a process Output into a single string.
fn captured_output(output: &std::process::Output) -> String {
    let mut buf = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        buf.push_str(&stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        buf.push_str(&stderr);
    }
    buf
}

/// Configure the worktree's branch to track `origin/<branch>` (a same-name
/// upstream) rather than inheriting the start-point's tracking. `git worktree
/// add -b` defaults to copying the start-point's upstream config — for a
/// session branch like `foo` started from `main`, that leaves the branch
/// tracking `origin/main`, which silently breaks `git push` (`push.default =
/// simple` refuses on name mismatch) and `git push --force-with-lease` (the
/// lease check then targets `origin/main`'s SHA, not the session branch's).
fn set_self_upstream(worktree_dir: &str, branch: &str) {
    let merge_ref = format!("refs/heads/{}", branch);
    let _ = Command::new("git")
        .args([
            "-C",
            worktree_dir,
            "config",
            &format!("branch.{}.remote", branch),
            "origin",
        ])
        .output();
    let _ = Command::new("git")
        .args([
            "-C",
            worktree_dir,
            "config",
            &format!("branch.{}.merge", branch),
            &merge_ref,
        ])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn run_git(args: &[&str]) {
        let s = Command::new("git").args(args).status().unwrap();
        assert!(s.success(), "git {:?} failed", args);
    }

    fn config_value(dir: &str, key: &str) -> String {
        let out = Command::new("git")
            .args(["-C", dir, "config", "--get", key])
            .output()
            .unwrap();
        assert!(out.status.success(), "git config --get {} failed", key);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn test_worktree_branch_tracks_itself_not_source_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let bare = home.join("repo.git");
        let bare_str = bare.to_str().unwrap();

        // Build a "remote" with a main branch, then clone --bare to mimic
        // box's repo setup.
        let remote = home.join("remote.git");
        run_git(&[
            "-c",
            "init.defaultBranch=main",
            "init",
            "--bare",
            remote.to_str().unwrap(),
        ]);
        let src = home.join("src");
        run_git(&[
            "-c",
            "init.defaultBranch=main",
            "clone",
            remote.to_str().unwrap(),
            src.to_str().unwrap(),
        ]);
        let src_str = src.to_str().unwrap();
        run_git(&[
            "-C",
            src_str,
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ]);
        run_git(&["-C", src_str, "push", "-q", "origin", "main"]);
        run_git(&["clone", "--bare", remote.to_str().unwrap(), bare_str]);

        // Mirror box's bare setup: BOTH the heads/heads refspec (which makes
        // `refs/heads/*` act as the remote-tracking namespace and is what
        // triggers git's `worktree add -b` to copy upstream forward) and the
        // origin/* refspec.
        run_git(&[
            "-C",
            bare_str,
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/heads/*",
        ]);
        run_git(&[
            "-C",
            bare_str,
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ]);
        run_git(&["-C", bare_str, "fetch", "-q", "origin"]);

        // Reproduce git's stock behavior: `git worktree add -b` from a bare
        // copies main's upstream config to the new branch.
        let wt = home.join("wt");
        run_git(&[
            "-C",
            bare_str,
            "worktree",
            "add",
            wt.to_str().unwrap(),
            "-b",
            "foo",
        ]);
        assert_eq!(
            config_value(wt.to_str().unwrap(), "branch.foo.merge"),
            "refs/heads/main",
            "precondition: stock git misconfigures upstream"
        );

        // After our fix runs, the branch should track itself.
        set_self_upstream(wt.to_str().unwrap(), "foo");
        assert_eq!(
            config_value(wt.to_str().unwrap(), "branch.foo.merge"),
            "refs/heads/foo"
        );
        assert_eq!(
            config_value(wt.to_str().unwrap(), "branch.foo.remote"),
            "origin"
        );

        // Sanity: the worktree's git common dir is the bare, so the config
        // wrote to the bare, not the worktree-only config.
        assert!(Path::new(bare_str).join("config").exists());
    }
}
