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

    /// Resolve strategy from an optional CLI value, falling back to BOX_STRATEGY env var,
    /// then defaulting to "worktree".
    pub fn resolve(cli_value: Option<&str>) -> Result<Self> {
        let value = match cli_value {
            Some(v) => v.to_string(),
            None => std::env::var("BOX_STRATEGY").unwrap_or_else(|_| "worktree".to_string()),
        };
        Strategy::from_str(&value)
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
/// the unified experience of `ensure_workspace_multi*`. For Worktree sessions
/// each unit runs `git worktree remove --force` + `git branch -D`; for Clone
/// sessions each unit is a per-repo `fs::remove_dir_all`. After the bar
/// finishes the now-empty workspace root dir for each session is cleaned up.
pub fn remove_sessions(sessions: &[(String, Strategy, Vec<String>)], verbose: bool) {
    let all_repos = crate::repo::list().unwrap_or_default();
    let root = match config::box_root() {
        Ok(r) => r,
        Err(_) => return,
    };

    enum Unit {
        Worktree {
            repo_path: Option<String>,
            dest: std::path::PathBuf,
            branch: String,
        },
        Clone {
            dest: std::path::PathBuf,
        },
    }

    let mut items: Vec<(String, Unit)> = Vec::new();
    let mut all_worktree = true;
    for (name, strategy, repo_names) in sessions {
        match strategy {
            Strategy::Worktree => {
                let branch = name.clone();
                for repo_name in repo_names {
                    let dest = root.join("workspaces").join(name).join(repo_name);
                    let repo_path = all_repos
                        .iter()
                        .find(|r| r.name == *repo_name)
                        .map(|r| r.path.clone());
                    items.push((
                        format!("{}/{}", name, repo_name),
                        Unit::Worktree {
                            repo_path,
                            dest,
                            branch: branch.clone(),
                        },
                    ));
                }
            }
            Strategy::Clone => {
                all_worktree = false;
                for repo_name in repo_names {
                    let dest = root.join("workspaces").join(name).join(repo_name);
                    items.push((format!("{}/{}", name, repo_name), Unit::Clone { dest }));
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
            |_name, unit| match unit {
                Unit::Worktree {
                    repo_path,
                    dest,
                    branch,
                } => {
                    let dest_str = dest.to_string_lossy().to_string();
                    if let Some(path) = repo_path {
                        let mut buf = String::new();
                        let mut success = true;
                        match Command::new("git")
                            .args(["-C", &path, "worktree", "remove", "--force", &dest_str])
                            .output()
                        {
                            Ok(o) => {
                                buf.push_str(&captured_output(&o));
                                if !o.status.success() {
                                    success = false;
                                }
                            }
                            Err(e) => {
                                success = false;
                                buf.push_str(&format!(
                                    "failed to run git worktree remove: {}\n",
                                    e
                                ));
                            }
                        }
                        match Command::new("git")
                            .args(["-C", &path, "branch", "-D", &branch])
                            .output()
                        {
                            Ok(o) => {
                                buf.push_str(&captured_output(&o));
                                if !o.status.success() {
                                    success = false;
                                }
                            }
                            Err(e) => {
                                success = false;
                                buf.push_str(&format!("failed to run git branch -D: {}\n", e));
                            }
                        }
                        (success, buf)
                    } else {
                        match std::fs::remove_dir_all(&dest) {
                            Ok(()) => (true, String::new()),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                (true, String::new())
                            }
                            Err(e) => (
                                false,
                                format!("failed to remove '{}': {}", dest.display(), e),
                            ),
                        }
                    }
                }
                Unit::Clone { dest } => match std::fs::remove_dir_all(&dest) {
                    Ok(()) => (true, String::new()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (true, String::new()),
                    Err(e) => (
                        false,
                        format!("failed to remove '{}': {}", dest.display(), e),
                    ),
                },
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
    }

    for (name, _, _) in sessions {
        let session_root = root.join("workspaces").join(name);
        let _ = std::fs::remove_dir_all(&session_root);
    }
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
    let dest_str = dest.to_string_lossy().to_string();

    if let Some(entry) = all_repos.iter().find(|r| r.name == repo_name) {
        let _ = Command::new("git")
            .args([
                "-C",
                &entry.path,
                "worktree",
                "remove",
                "--force",
                &dest_str,
            ])
            .status();
        let _ = Command::new("git")
            .args(["-C", &entry.path, "branch", "-D", &branch_name])
            .status();
    } else {
        let _ = std::fs::remove_dir_all(&dest);
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
