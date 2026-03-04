use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;
use crate::git;

#[derive(Debug, Clone)]
pub struct RepoEntry {
    pub name: String,
    pub path: String,
}

pub fn repos_file() -> Result<PathBuf> {
    let home = config::home_dir()?;
    Ok(PathBuf::from(home).join(".box").join("repos"))
}

pub fn list() -> Result<Vec<RepoEntry>> {
    let file = repos_file()?;
    if !file.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&file)?;
    let entries = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let path = l.trim().to_string();
            let name = Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            RepoEntry { name, path }
        })
        .collect();
    Ok(entries)
}

pub fn add(path: &str) -> Result<()> {
    let canonical =
        fs::canonicalize(path).map_err(|_| anyhow::anyhow!("Path '{}' does not exist.", path))?;
    let canonical_str = canonical.to_string_lossy().to_string();

    if !git::is_repo(&canonical) {
        bail!("'{}' is not a git repository.", canonical_str);
    }

    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("Cannot derive repo name from path."))?;

    let existing = list()?;
    for entry in &existing {
        if entry.path == canonical_str {
            bail!("Repo '{}' is already registered.", canonical_str);
        }
        if entry.name == name {
            bail!(
                "A repo named '{}' is already registered (from '{}').",
                name,
                entry.path
            );
        }
    }

    let file = repos_file()?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = if file.exists() {
        fs::read_to_string(&file)?
    } else {
        String::new()
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&canonical_str);
    content.push('\n');
    fs::write(&file, content)?;

    eprintln!(
        "Registered repo '\x1b[1m{}\x1b[0m' ({})",
        name, canonical_str
    );
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    let existing = list()?;
    let found = existing.iter().any(|e| e.name == name);
    if !found {
        bail!("No repo named '{}' is registered.", name);
    }

    let file = repos_file()?;
    let content: String = existing
        .iter()
        .filter(|e| e.name != name)
        .map(|e| e.path.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        String::new()
    } else {
        format!("{}\n", content)
    };
    fs::write(&file, content)?;

    eprintln!("Removed repo '{}'.", name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F: FnOnce(&Path)>(f: F) {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        f(tmp.path());
        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn make_git_repo(base: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git")
            .args(["init", dir.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
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
        });
    }

    #[test]
    fn test_add_duplicate_path() {
        with_temp_home(|home| {
            let repo = make_git_repo(home, "my-app");
            add(repo.to_str().unwrap()).unwrap();
            let err = add(repo.to_str().unwrap()).unwrap_err();
            assert!(err.to_string().contains("already registered"));
        });
    }

    #[test]
    fn test_add_duplicate_name() {
        with_temp_home(|home| {
            let dir1 = home.join("a");
            fs::create_dir_all(&dir1).unwrap();
            let repo1 = make_git_repo(&dir1, "app");
            add(repo1.to_str().unwrap()).unwrap();

            let dir2 = home.join("b");
            fs::create_dir_all(&dir2).unwrap();
            let repo2 = make_git_repo(&dir2, "app");
            let err = add(repo2.to_str().unwrap()).unwrap_err();
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
}
