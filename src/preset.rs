use anyhow::{bail, Result};
use std::fs;
use std::path::PathBuf;

use crate::config;
use crate::repo;

pub fn presets_dir() -> Result<PathBuf> {
    Ok(config::box_root()?.join("presets"))
}

pub fn list() -> Result<Vec<(String, Vec<String>)>> {
    let dir = presets_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        let repos: Vec<String> = content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        if !repos.is_empty() {
            entries.push((name, repos));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

pub fn load(name: &str) -> Result<Vec<String>> {
    validate_name(name)?;
    let path = presets_dir()?.join(name);
    if !path.is_file() {
        bail!("No preset named '{}' found.", name);
    }
    let content = fs::read_to_string(&path)?;
    let repos: Vec<String> = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    Ok(repos)
}

pub fn add(name: &str, repos: &[String]) -> Result<()> {
    validate_name(name)?;
    if repos.is_empty() {
        bail!("A preset must contain at least one repo.");
    }

    // Deduplicate, preserving first occurrence order.
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&str> = repos
        .iter()
        .map(|r| r.as_str())
        .filter(|r| seen.insert(*r))
        .collect();

    let all_repos = repo::list()?;
    let registered: std::collections::HashSet<&str> =
        all_repos.iter().map(|r| r.name.as_str()).collect();
    for r in &unique {
        if !registered.contains(r) {
            bail!("Repo '{}' not found in registry.", r);
        }
    }

    let dir = presets_dir()?;
    fs::create_dir_all(&dir)?;

    let path = dir.join(name);
    let exists = path.is_file();
    fs::write(&path, unique.join("\n") + "\n")?;

    let verb = if exists { "updated" } else { "saved" };
    eprintln!(
        "Preset '\x1b[1m{}\x1b[0m' {} ({}).",
        name,
        verb,
        unique.join(", ")
    );
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    validate_name(name)?;
    let path = presets_dir()?.join(name);
    if !path.is_file() {
        bail!("No preset named '{}' found.", name);
    }
    fs::remove_file(&path)?;
    eprintln!("Removed preset '{}'.", name);
    Ok(())
}

/// Load a preset and validate that all referenced repos still exist.
/// Warns on stderr for missing repos and filters them out.
pub fn resolve(name: &str) -> Result<Vec<String>> {
    let repos = load(name)?;
    let all_repos = repo::list()?;
    let registered: std::collections::HashSet<&str> =
        all_repos.iter().map(|r| r.name.as_str()).collect();

    let mut valid = Vec::new();
    for r in &repos {
        if registered.contains(r.as_str()) {
            valid.push(r.clone());
        } else {
            eprintln!(
                "Warning: preset '{}' references removed repo '{}', skipping.",
                name, r
            );
        }
    }
    if valid.is_empty() {
        bail!(
            "Preset '{}' has no valid repos (all referenced repos have been removed).",
            name
        );
    }
    Ok(valid)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        bail!("Invalid preset name '{}'.", name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ENV_LOCK;
    use std::path::Path;

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

    fn make_git_repo(base: &Path, name: &str) -> std::path::PathBuf {
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
            let presets = list().unwrap();
            assert!(presets.is_empty());
        });
    }

    #[test]
    fn test_add_and_list() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "app-a");
            repo::add(repo.to_str().unwrap()).unwrap();
            let repo = make_git_repo(home, "app-b");
            repo::add(repo.to_str().unwrap()).unwrap();

            add("work", &["app-a".to_string(), "app-b".to_string()]).unwrap();

            let presets = list().unwrap();
            assert_eq!(presets.len(), 1);
            assert_eq!(presets[0].0, "work");
            assert_eq!(presets[0].1, vec!["app-a", "app-b"]);
        });
    }

    #[test]
    fn test_add_validates_repos_exist() {
        with_temp_home(|_| {
            let err = add("work", &["nonexistent".to_string()]).unwrap_err();
            assert!(err.to_string().contains("not found in registry"));
        });
    }

    #[test]
    fn test_add_empty_repos_fails() {
        with_temp_home(|_| {
            let err = add("work", &[]).unwrap_err();
            assert!(err.to_string().contains("at least one repo"));
        });
    }

    #[test]
    fn test_add_overwrites() {
        with_temp_home(|home| {
            let repo_a = make_git_repo(home, "app-a");
            let repo_b = make_git_repo(home, "app-b");
            repo::add(repo_a.to_str().unwrap()).unwrap();
            repo::add(repo_b.to_str().unwrap()).unwrap();

            add("work", &["app-a".to_string()]).unwrap();
            add("work", &["app-b".to_string()]).unwrap();

            let repos = load("work").unwrap();
            assert_eq!(repos, vec!["app-b"]);
        });
    }

    #[test]
    fn test_remove() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "app-a");
            repo::add(repo.to_str().unwrap()).unwrap();
            add("work", &["app-a".to_string()]).unwrap();

            remove("work").unwrap();
            let presets = list().unwrap();
            assert!(presets.is_empty());
        });
    }

    #[test]
    fn test_remove_not_found() {
        with_temp_home(|_| {
            let err = remove("nonexistent").unwrap_err();
            assert!(err.to_string().contains("No preset named"));
        });
    }

    #[test]
    fn test_load() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "app-a");
            repo::add(repo.to_str().unwrap()).unwrap();
            add("work", &["app-a".to_string()]).unwrap();

            let repos = load("work").unwrap();
            assert_eq!(repos, vec!["app-a"]);
        });
    }

    #[test]
    fn test_resolve_filters_missing_repos() {
        with_temp_home(|home| {
            let repo_a = make_git_repo(home, "app-a");
            let repo_b = make_git_repo(home, "app-b");
            repo::add(repo_a.to_str().unwrap()).unwrap();
            repo::add(repo_b.to_str().unwrap()).unwrap();
            add("work", &["app-a".to_string(), "app-b".to_string()]).unwrap();

            // Remove one repo
            repo::remove("app-b").unwrap();

            let repos = resolve("work").unwrap();
            assert_eq!(repos, vec!["app-a"]);
        });
    }

    #[test]
    fn test_resolve_all_repos_missing_fails() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "app-a");
            repo::add(repo.to_str().unwrap()).unwrap();
            add("work", &["app-a".to_string()]).unwrap();

            repo::remove("app-a").unwrap();

            let err = resolve("work").unwrap_err();
            assert!(err.to_string().contains("no valid repos"));
        });
    }

    #[test]
    fn test_path_traversal_rejected() {
        with_temp_home(|_| {
            assert!(add("../evil", &["a".to_string()]).is_err());
            assert!(add("foo/bar", &["a".to_string()]).is_err());
            assert!(add(".", &["a".to_string()]).is_err());
            assert!(add("..", &["a".to_string()]).is_err());
            assert!(add(".hidden", &["a".to_string()]).is_err());
            assert!(load("../evil").is_err());
            assert!(load(".hidden").is_err());
            assert!(remove("../evil").is_err());
        });
    }
}
