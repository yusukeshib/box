use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDateTime, Utc};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Clone)]
pub struct Session {
    pub name: String,
    pub project_dir: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub repos: Vec<String>,
}

impl From<config::BoxConfig> for Session {
    fn from(cfg: config::BoxConfig) -> Self {
        Session {
            name: cfg.name,
            project_dir: cfg.project_dir,
            command: cfg.command,
            env: cfg.env,
            repos: cfg.repos,
        }
    }
}

#[derive(Clone)]
pub struct SessionSummary {
    pub name: String,
    pub project_dir: String,
    pub command: String,
    pub created_at: String,
    pub repos: Vec<String>,
}

pub fn sessions_dir() -> Result<PathBuf> {
    Ok(config::box_root()?.join("sessions"))
}

const RESERVED_NAMES: &[&str] = &[
    "new", "remove", "exec", "edit", "upgrade", "path", "config", "list", "ls", "repo",
];

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Session name is required.");
    }
    if name.contains('/') {
        bail!(
            "Invalid session name '{}'. The '/' character is not allowed.",
            name
        );
    }
    if RESERVED_NAMES.contains(&name) {
        bail!(
            "'{}' is a reserved name and cannot be used as a session name.",
            name
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "Invalid session name '{}'. Use only letters, digits, hyphens, and underscores.",
            name
        );
    }
    Ok(())
}

pub fn session_exists(name: &str) -> Result<bool> {
    let dir = sessions_dir()?.join(name);
    Ok(dir.join("project_dir").exists() || dir.join("repos").exists())
}

pub fn save(session: &Session) -> Result<()> {
    let dir = sessions_dir()?.join(&session.name);
    fs::create_dir_all(&dir).context("Failed to create session directory")?;
    #[cfg(unix)]
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    if !session.project_dir.is_empty() {
        fs::write(dir.join("project_dir"), &session.project_dir)?;
    }
    fs::write(
        dir.join("created_at"),
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )?;
    if !session.command.is_empty() {
        let content: Vec<&str> = session.command.iter().map(|s| s.as_str()).collect();
        fs::write(dir.join("command"), content.join("\0"))?;
    } else {
        let _ = fs::remove_file(dir.join("command"));
    }
    if !session.env.is_empty() {
        let content: Vec<&str> = session.env.iter().map(|s| s.as_str()).collect();
        fs::write(dir.join("env"), content.join("\0"))?;
    } else {
        let _ = fs::remove_file(dir.join("env"));
    }
    if !session.repos.is_empty() {
        fs::write(dir.join("repos"), session.repos.join("\n"))?;
    } else {
        let _ = fs::remove_file(dir.join("repos"));
    }
    Ok(())
}

/// Migrate a nested `sessions/<name>/default/` layout to flat `sessions/<name>/`.
/// Moves files from `default/` up and removes the subdirectory.
fn migrate_nested_to_flat(name: &str) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    let default_dir = dir.join("default");
    if !default_dir.is_dir() || !default_dir.join("project_dir").exists() {
        return Ok(());
    }
    // Move all files from default/ up to the session dir
    for entry in fs::read_dir(&default_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let file_name = entry.file_name();
            fs::rename(&path, dir.join(&file_name))?;
        }
    }
    // Remove the now-empty default/ subdir (and any other session subdirs)
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

pub fn load(name: &str) -> Result<Session> {
    let dir = sessions_dir()?.join(name);

    // Auto-migrate nested session on load
    if !dir.join("project_dir").exists() && dir.join("default").join("project_dir").exists() {
        let _ = migrate_nested_to_flat(name);
    }

    if !dir.is_dir() {
        bail!("Session '{}' not found.", name);
    }

    let project_dir_path = dir.join("project_dir");
    let project_dir = if project_dir_path.exists() {
        fs::read_to_string(&project_dir_path)?.trim().to_string()
    } else if dir.join("repos").exists() {
        // Multi-repo session without a single project_dir
        String::new()
    } else {
        bail!("Session '{}' is missing project directory metadata.", name);
    };

    let command = fs::read_to_string(dir.join("command"))
        .map(|s| {
            s.split('\0')
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default();

    let env = fs::read_to_string(dir.join("env"))
        .map(|s| {
            s.split('\0')
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default();

    let repos = fs::read_to_string(dir.join("repos"))
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default();

    Ok(Session {
        name: name.to_string(),
        project_dir,
        command,
        env,
        repos,
    })
}

fn read_session_summary(session_path: &std::path::Path, name: String) -> SessionSummary {
    let project_dir = fs::read_to_string(session_path.join("project_dir"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let created_at = fs::read_to_string(session_path.join("created_at"))
        .map(|s| {
            let trimmed = s.trim();
            if let Some(naive_str) = trimmed.strip_suffix(" UTC") {
                if let Ok(naive) = NaiveDateTime::parse_from_str(naive_str, "%Y-%m-%d %H:%M:%S") {
                    let utc_dt = naive.and_utc();
                    let local_dt = utc_dt.with_timezone(&Local);
                    return local_dt.format("%Y-%m-%d %H:%M:%S %Z").to_string();
                }
            }
            trimmed.to_string()
        })
        .unwrap_or_default();
    let command = fs::read_to_string(session_path.join("command"))
        .map(|s| {
            s.split('\0')
                .filter(|l| !l.is_empty())
                .filter(|l| *l != "--allow-dangerously-skip-permissions")
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    let repos: Vec<String> = fs::read_to_string(session_path.join("repos"))
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default();

    SessionSummary {
        name,
        project_dir,
        command,
        created_at,
        repos,
    }
}

pub fn list() -> Result<Vec<SessionSummary>> {
    let dir = sessions_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        // Auto-migrate nested sessions/<name>/default/ → sessions/<name>/
        if !path.join("project_dir").exists() && path.join("default").join("project_dir").exists() {
            let _ = migrate_nested_to_flat(&name);
        }

        if path.join("project_dir").exists() || path.join("repos").exists() {
            sessions.push(read_session_summary(&path, name));
        }
    }

    Ok(sessions)
}

pub fn update_repos(name: &str, repos: &[String]) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    if !dir.is_dir() {
        bail!("Session '{}' not found.", name);
    }
    if repos.is_empty() {
        let _ = fs::remove_file(dir.join("repos"));
    } else {
        fs::write(dir.join("repos"), repos.join("\n"))?;
    }
    Ok(())
}

pub fn remove_dir(name: &str) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    fs::remove_dir_all(&dir).context(format!("Failed to remove session directory for '{}'", name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ENV_LOCK;

    fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
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

    fn make_session(name: &str, project_dir: &str) -> Session {
        Session {
            name: name.to_string(),
            project_dir: project_dir.to_string(),
            command: vec![],
            env: vec![],
            repos: vec![],
        }
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("my-session").is_ok());
        assert!(validate_name("test_123").is_ok());
        assert!(validate_name("a").is_ok());
        assert!(validate_name("ABC").is_ok());
        assert!(validate_name("hello-world_99").is_ok());
    }

    #[test]
    fn test_validate_name_with_slash() {
        let err = validate_name("my-feature/server").unwrap_err();
        assert!(err.to_string().contains("'/' character is not allowed"));
    }

    #[test]
    fn test_validate_name_empty() {
        let err = validate_name("").unwrap_err();
        assert_eq!(err.to_string(), "Session name is required.");
    }

    #[test]
    fn test_validate_name_reserved() {
        let err = validate_name("upgrade").unwrap_err();
        assert!(err.to_string().contains("reserved name"));
    }

    #[test]
    fn test_validate_name_reserved_path() {
        let err = validate_name("path").unwrap_err();
        assert!(err.to_string().contains("reserved name"));
    }

    #[test]
    fn test_validate_name_reserved_config() {
        let err = validate_name("config").unwrap_err();
        assert!(err.to_string().contains("reserved name"));
    }

    #[test]
    fn test_validate_name_reserved_repo() {
        let err = validate_name("repo").unwrap_err();
        assert!(err.to_string().contains("reserved name"));
    }

    #[test]
    fn test_validate_name_invalid_chars() {
        let err = validate_name("bad name").unwrap_err();
        assert!(err.to_string().contains("Invalid session name"));
        assert!(err.to_string().contains("bad name"));

        let err = validate_name("bad.name").unwrap_err();
        assert!(err.to_string().contains("Invalid session name"));

        let err = validate_name("bad@name").unwrap_err();
        assert!(err.to_string().contains("Invalid session name"));
    }

    #[test]
    fn test_sessions_dir() {
        with_temp_home(|tmp| {
            let dir = sessions_dir().unwrap();
            assert_eq!(dir, tmp.join(".box").join("sessions"));
        });
    }

    #[test]
    fn test_save_and_load_basic() {
        with_temp_home(|_| {
            let sess = make_session("test-ws", "/tmp/myproject");
            save(&sess).unwrap();

            let loaded = load("test-ws").unwrap();
            assert_eq!(loaded.name, "test-ws");
            assert_eq!(loaded.project_dir, "/tmp/myproject");
            assert!(loaded.command.is_empty());
        });
    }

    #[test]
    fn test_save_and_load_with_command() {
        with_temp_home(|_| {
            let sess = Session {
                name: "full-ws".to_string(),
                project_dir: "/tmp/project".to_string(),
                command: vec![
                    "bash".to_string(),
                    "-c".to_string(),
                    "echo hello".to_string(),
                ],
                env: vec![],
                repos: vec![],
            };
            save(&sess).unwrap();

            let loaded = load("full-ws").unwrap();
            assert_eq!(loaded.command, vec!["bash", "-c", "echo hello"]);
        });
    }

    #[test]
    fn test_save_creates_metadata_files() {
        with_temp_home(|_| {
            let sess = make_session("meta-test", "/tmp/p");
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("meta-test");
            assert!(dir.join("project_dir").exists());
            assert!(dir.join("created_at").exists());
            assert!(!dir.join("command").exists());

            let created = fs::read_to_string(dir.join("created_at")).unwrap();
            assert!(created.ends_with("UTC"));
        });
    }

    #[test]
    fn test_load_nonexistent() {
        with_temp_home(|_| {
            let err = load("nonexistent").unwrap_err();
            assert_eq!(err.to_string(), "Session 'nonexistent' not found.");
        });
    }

    #[test]
    fn test_load_missing_project_dir() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("broken");
            fs::create_dir_all(&dir).unwrap();

            let err = load("broken").unwrap_err();
            assert!(err
                .to_string()
                .contains("missing project directory metadata"));
        });
    }

    #[test]
    fn test_load_defaults_when_optional_files_missing() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("minimal");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "/tmp/project").unwrap();

            let loaded = load("minimal").unwrap();
            assert_eq!(loaded.project_dir, "/tmp/project");
        });
    }

    #[test]
    fn test_session_exists() {
        with_temp_home(|_| {
            assert!(!session_exists("nope").unwrap());

            let sess = make_session("exists-test", "/tmp/p");
            save(&sess).unwrap();
            assert!(session_exists("exists-test").unwrap());
        });
    }

    #[test]
    fn test_list_empty() {
        with_temp_home(|_| {
            let sessions = list().unwrap();
            assert!(sessions.is_empty());
        });
    }

    #[test]
    fn test_list_multiple_sessions() {
        with_temp_home(|_| {
            for name in &["alpha", "beta", "gamma"] {
                let sess = make_session(name, &format!("/tmp/{}", name));
                save(&sess).unwrap();
            }

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 3);
            assert_eq!(sessions[0].name, "alpha");
            assert_eq!(sessions[1].name, "beta");
            assert_eq!(sessions[2].name, "gamma");
        });
    }

    #[test]
    fn test_list_reads_metadata() {
        with_temp_home(|_| {
            let sess = make_session("list-meta", "/home/user/project");
            save(&sess).unwrap();

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].project_dir, "/home/user/project");
            assert!(!sessions[0].created_at.is_empty());
        });
    }

    #[test]
    fn test_remove_dir() {
        with_temp_home(|_| {
            let sess = make_session("to-remove", "/tmp/p");
            save(&sess).unwrap();
            assert!(session_exists("to-remove").unwrap());

            remove_dir("to-remove").unwrap();
            assert!(!session_exists("to-remove").unwrap());
        });
    }

    #[test]
    fn test_remove_dir_nonexistent() {
        with_temp_home(|_| {
            let err = remove_dir("nonexistent").unwrap_err();
            assert!(err.to_string().contains("Failed to remove"));
        });
    }

    #[test]
    fn test_save_trims_whitespace_on_load() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("trim-test");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "  /tmp/project  \n").unwrap();

            let loaded = load("trim-test").unwrap();
            assert_eq!(loaded.project_dir, "/tmp/project");
        });
    }

    #[test]
    fn test_command_save_format() {
        with_temp_home(|_| {
            let sess = Session {
                name: "cmd-format".to_string(),
                project_dir: "/tmp/p".to_string(),
                command: vec!["bash".to_string(), "-c".to_string(), "echo hi".to_string()],
                env: vec![],
                repos: vec![],
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("cmd-format");
            let raw = fs::read_to_string(dir.join("command")).unwrap();
            assert_eq!(raw, "bash\0-c\0echo hi");
        });
    }

    #[test]
    fn test_save_and_load_with_env() {
        with_temp_home(|_| {
            let sess = Session {
                name: "env-test".to_string(),
                project_dir: "/tmp/project".to_string(),
                command: vec![],
                env: vec!["FOO=bar".to_string(), "BAZ".to_string()],
                repos: vec![],
            };
            save(&sess).unwrap();

            let loaded = load("env-test").unwrap();
            assert_eq!(loaded.env, vec!["FOO=bar", "BAZ"]);

            let dir = sessions_dir().unwrap().join("env-test");
            let raw = fs::read_to_string(dir.join("env")).unwrap();
            assert_eq!(raw, "FOO=bar\0BAZ");
        });
    }

    #[test]
    fn test_save_and_load_empty_env() {
        with_temp_home(|_| {
            let sess = make_session("no-env", "/tmp/project");
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("no-env");
            assert!(!dir.join("env").exists());

            let loaded = load("no-env").unwrap();
            assert!(loaded.env.is_empty());
        });
    }

    #[test]
    fn test_save_and_load_with_repos() {
        with_temp_home(|_| {
            let sess = Session {
                name: "multi".to_string(),
                project_dir: String::new(),
                command: vec![],
                env: vec![],
                repos: vec!["app-a".to_string(), "app-b".to_string()],
            };
            save(&sess).unwrap();

            let loaded = load("multi").unwrap();
            assert_eq!(loaded.repos, vec!["app-a", "app-b"]);
            assert!(loaded.project_dir.is_empty());
            assert!(session_exists("multi").unwrap());
        });
    }

    #[test]
    fn test_migration_nested_to_flat_on_list() {
        with_temp_home(|_| {
            // Simulate old nested layout: sessions/<name>/default/project_dir
            let dir = sessions_dir().unwrap().join("old-session");
            let default_dir = dir.join("default");
            fs::create_dir_all(&default_dir).unwrap();
            fs::write(default_dir.join("project_dir"), "/tmp/project").unwrap();

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].name, "old-session");

            // Files should have been migrated up
            assert!(dir.join("project_dir").exists());
            assert!(!default_dir.exists());
        });
    }

    #[test]
    fn test_migration_nested_to_flat_on_load() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("old-load");
            let default_dir = dir.join("default");
            fs::create_dir_all(&default_dir).unwrap();
            fs::write(default_dir.join("project_dir"), "/tmp/project").unwrap();

            let loaded = load("old-load").unwrap();
            assert_eq!(loaded.name, "old-load");
            assert_eq!(loaded.project_dir, "/tmp/project");
        });
    }
}
