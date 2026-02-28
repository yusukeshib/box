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
    pub label: Option<String>,
    pub project_dir: String,
    pub image: String,
    pub mount_path: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub local: bool,
    pub color: Option<String>,
    pub strategy: String,
}

impl Session {
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

impl From<config::BoxConfig> for Session {
    fn from(cfg: config::BoxConfig) -> Self {
        Session {
            name: cfg.name,
            label: cfg.label,
            project_dir: cfg.project_dir,
            image: cfg.image,
            mount_path: cfg.mount_path,
            command: cfg.command,
            env: cfg.env,
            local: cfg.local,
            color: cfg.color,
            strategy: cfg.strategy,
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct SessionSummary {
    pub name: String,
    pub label: Option<String>,
    pub project_dir: String,
    pub image: String,
    pub command: String,
    pub created_at: String,
    pub running: bool,
    pub local: bool,
    pub color: Option<String>,
    pub strategy: String,
}

impl SessionSummary {
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = PathBuf::from(config::home_dir()?)
        .join(".box")
        .join("sessions");
    Ok(dir)
}

pub fn normalize_name(name: &str) -> String {
    let normalized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut result = String::new();
    for c in normalized.chars() {
        if c == '-' && result.ends_with('-') {
            continue;
        }
        result.push(c);
    }
    result.trim_matches('-').to_string()
}

const RESERVED_NAMES: &[&str] = &[
    "create", "resume", "remove", "stop", "exec", "upgrade", "path", "config", "list", "ls",
];

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Session name is required.");
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
    Ok(sessions_dir()?.join(name).is_dir())
}

pub fn save(session: &Session) -> Result<()> {
    let dir = sessions_dir()?.join(&session.name);
    fs::create_dir_all(&dir).context("Failed to create session directory")?;
    // Restrict session directory to owner-only access (0o700) to prevent
    // other local users from connecting to the Unix socket or tampering
    // with PID files.
    #[cfg(unix)]
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    fs::write(dir.join("project_dir"), &session.project_dir)?;
    fs::write(dir.join("image"), &session.image)?;
    fs::write(dir.join("mount_path"), &session.mount_path)?;
    fs::write(
        dir.join("mode"),
        if session.local { "local" } else { "docker" },
    )?;
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
    if let Some(ref label) = session.label {
        fs::write(dir.join("label"), label)?;
    } else {
        let _ = fs::remove_file(dir.join("label"));
    }
    if let Some(ref color) = session.color {
        fs::write(dir.join("color"), color)?;
    } else {
        let _ = fs::remove_file(dir.join("color"));
    }
    fs::write(dir.join("strategy"), &session.strategy)?;
    Ok(())
}

pub fn load(name: &str) -> Result<Session> {
    let dir = sessions_dir()?.join(name);
    if !dir.is_dir() {
        bail!("Session '{}' not found.", name);
    }

    let project_dir_path = dir.join("project_dir");
    if !project_dir_path.exists() {
        bail!("Session '{}' is missing project directory metadata.", name);
    }
    let project_dir = fs::read_to_string(&project_dir_path)?.trim().to_string();

    let image = fs::read_to_string(dir.join("image"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| config::DEFAULT_IMAGE.to_string());

    let mount_path = fs::read_to_string(dir.join("mount_path"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| config::derive_mount_path(&project_dir));

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

    let local = fs::read_to_string(dir.join("mode"))
        .map(|s| s.trim() == "local")
        .unwrap_or(false);

    let label = fs::read_to_string(dir.join("label"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let color = fs::read_to_string(dir.join("color"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let strategy = fs::read_to_string(dir.join("strategy"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "clone".to_string());

    Ok(Session {
        name: name.to_string(),
        label,
        project_dir,
        image,
        mount_path,
        command,
        env,
        local,
        color,
        strategy,
    })
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
        let session_path = entry.path();

        let project_dir = fs::read_to_string(session_path.join("project_dir"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let image = fs::read_to_string(session_path.join("image"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let created_at = fs::read_to_string(session_path.join("created_at"))
            .map(|s| {
                let trimmed = s.trim();
                if let Some(naive_str) = trimmed.strip_suffix(" UTC") {
                    if let Ok(naive) = NaiveDateTime::parse_from_str(naive_str, "%Y-%m-%d %H:%M:%S")
                    {
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

        let local = fs::read_to_string(session_path.join("mode"))
            .map(|s| s.trim() == "local")
            .unwrap_or(false);

        let label = fs::read_to_string(session_path.join("label"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let color = fs::read_to_string(session_path.join("color"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let strategy = fs::read_to_string(session_path.join("strategy"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "clone".to_string());

        sessions.push(SessionSummary {
            name,
            label,
            project_dir,
            image,
            command,
            created_at,
            running: false,
            local,
            color,
            strategy,
        });
    }

    Ok(sessions)
}

pub fn remove_dir(name: &str) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    fs::remove_dir_all(&dir).context(format!("Failed to remove session directory for '{}'", name))
}

pub fn touch_resumed_at(name: &str) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    fs::write(
        dir.join("resumed_at"),
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )?;
    Ok(())
}

pub fn write_pid(name: &str, pid: u32) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    fs::write(dir.join("pid"), pid.to_string())?;
    Ok(())
}

pub fn remove_pid(name: &str) {
    if let Ok(dir) = sessions_dir() {
        let _ = fs::remove_file(dir.join(name).join("pid"));
    }
}

pub fn socket_path(name: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(name).join("sock"))
}

pub fn remove_socket(name: &str) {
    if let Ok(dir) = sessions_dir() {
        let _ = fs::remove_file(dir.join(name).join("sock"));
    }
}

pub fn is_local_running(name: &str) -> bool {
    // Check socket first (authoritative if server is up)
    if let Ok(path) = socket_path(name) {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return true;
        }
    }
    // Fallback: PID check (handles server startup race)
    let dir = match sessions_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let pid_str = match fs::read_to_string(dir.join(name).join("pid")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let pid: i32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate HOME env var
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
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

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("my-session").is_ok());
        assert!(validate_name("test_123").is_ok());
        assert!(validate_name("a").is_ok());
        assert!(validate_name("ABC").is_ok());
        assert!(validate_name("hello-world_99").is_ok());
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
    fn test_validate_name_invalid_chars() {
        let err = validate_name("bad name").unwrap_err();
        assert!(err.to_string().contains("Invalid session name"));
        assert!(err.to_string().contains("bad name"));

        let err = validate_name("bad/name").unwrap_err();
        assert!(err.to_string().contains("Invalid session name"));

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
            let sess = Session {
                name: "test-session".to_string(),
                label: None,
                project_dir: "/tmp/myproject".to_string(),
                image: "ubuntu:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("test-session").unwrap();
            assert_eq!(loaded.name, "test-session");
            assert_eq!(loaded.project_dir, "/tmp/myproject");
            assert_eq!(loaded.image, "ubuntu:latest");
            assert_eq!(loaded.mount_path, "/workspace");
            assert!(loaded.command.is_empty());
        });
    }

    #[test]
    fn test_save_and_load_with_command() {
        with_temp_home(|_| {
            let sess = Session {
                name: "full-session".to_string(),
                label: None,
                project_dir: "/tmp/project".to_string(),
                image: "box-full:latest".to_string(),
                mount_path: "/src".to_string(),
                command: vec![
                    "bash".to_string(),
                    "-c".to_string(),
                    "echo hello".to_string(),
                ],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("full-session").unwrap();
            assert_eq!(loaded.command, vec!["bash", "-c", "echo hello"]);
        });
    }

    #[test]
    fn test_save_creates_metadata_files() {
        with_temp_home(|_| {
            let sess = Session {
                name: "meta-test".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("meta-test");
            assert!(dir.join("project_dir").exists());
            assert!(dir.join("image").exists());
            assert!(dir.join("mount_path").exists());
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
            // Don't write project_dir file

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
            // Don't write image or mount_path

            let loaded = load("minimal").unwrap();
            assert_eq!(loaded.image, config::DEFAULT_IMAGE);
            assert_eq!(loaded.mount_path, config::derive_mount_path("/tmp/project"));
        });
    }

    #[test]
    fn test_session_exists() {
        with_temp_home(|_| {
            assert!(!session_exists("nope").unwrap());

            let sess = Session {
                name: "exists-test".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
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
                let sess = Session {
                    name: name.to_string(),
                    label: None,
                    project_dir: format!("/tmp/{}", name),
                    image: "alpine:latest".to_string(),
                    mount_path: "/workspace".to_string(),
                    command: vec![],
                    env: vec![],
                    local: false,
                    color: None,
                    strategy: "clone".to_string(),
                };
                save(&sess).unwrap();
            }

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 3);
            // Should be sorted alphabetically
            assert_eq!(sessions[0].name, "alpha");
            assert_eq!(sessions[1].name, "beta");
            assert_eq!(sessions[2].name, "gamma");
        });
    }

    #[test]
    fn test_list_reads_metadata() {
        with_temp_home(|_| {
            let sess = Session {
                name: "list-meta".to_string(),
                label: None,
                project_dir: "/home/user/project".to_string(),
                image: "ubuntu:22.04".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].project_dir, "/home/user/project");
            assert_eq!(sessions[0].image, "ubuntu:22.04");
            assert!(!sessions[0].created_at.is_empty());
        });
    }

    #[test]
    fn test_remove_dir() {
        with_temp_home(|_| {
            let sess = Session {
                name: "to-remove".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
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
    fn test_touch_resumed_at() {
        with_temp_home(|_| {
            let sess = Session {
                name: "resume-test".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            touch_resumed_at("resume-test").unwrap();

            let dir = sessions_dir().unwrap().join("resume-test");
            let content = fs::read_to_string(dir.join("resumed_at")).unwrap();
            assert!(content.ends_with("UTC"));
        });
    }

    #[test]
    fn test_save_trims_whitespace_on_load() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("trim-test");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "  /tmp/project  \n").unwrap();
            fs::write(dir.join("image"), " ubuntu:latest \n").unwrap();
            fs::write(dir.join("mount_path"), " /src \n").unwrap();

            let loaded = load("trim-test").unwrap();
            assert_eq!(loaded.project_dir, "/tmp/project");
            assert_eq!(loaded.image, "ubuntu:latest");
            assert_eq!(loaded.mount_path, "/src");
        });
    }

    #[test]
    fn test_command_save_format() {
        with_temp_home(|_| {
            let sess = Session {
                name: "cmd-format".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec!["bash".to_string(), "-c".to_string(), "echo hi".to_string()],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
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
                label: None,
                project_dir: "/tmp/project".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec!["FOO=bar".to_string(), "BAZ".to_string()],
                local: false,
                color: None,
                strategy: "clone".to_string(),
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
            let sess = Session {
                name: "no-env".to_string(),
                label: None,
                project_dir: "/tmp/project".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("no-env");
            assert!(!dir.join("env").exists());

            let loaded = load("no-env").unwrap();
            assert!(loaded.env.is_empty());
        });
    }

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_name("yusuke/feature-1"), "yusuke-feature-1");
        assert_eq!(normalize_name("feature (1)"), "feature-1");
        assert_eq!(normalize_name("my.branch.name"), "my-branch-name");
        assert_eq!(normalize_name("fix#123"), "fix-123");
        assert_eq!(normalize_name("test$var!"), "test-var");
        assert_eq!(normalize_name("!!!"), "");
        assert_eq!(normalize_name("a--b"), "a-b"); // consecutive hyphens collapse
        assert_eq!(normalize_name("hello world"), "hello-world");
        assert_eq!(normalize_name("a/b/c"), "a-b-c");
        assert_eq!(normalize_name("already-valid"), "already-valid");
        assert_eq!(normalize_name("under_score"), "under_score");
        assert_eq!(normalize_name("---leading"), "leading");
        assert_eq!(normalize_name("trailing---"), "trailing");
    }

    #[test]
    fn test_save_and_load_with_label() {
        with_temp_home(|_| {
            let sess = Session {
                name: "normalized-name".to_string(),
                label: Some("original/name".to_string()),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("normalized-name").unwrap();
            assert_eq!(loaded.name, "normalized-name");
            assert_eq!(loaded.label.as_deref(), Some("original/name"));
            assert_eq!(loaded.display_name(), "original/name");
        });
    }

    #[test]
    fn test_save_and_load_without_label() {
        with_temp_home(|_| {
            let sess = Session {
                name: "no-label".to_string(),
                label: None,
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("no-label").unwrap();
            assert_eq!(loaded.label, None);
            assert_eq!(loaded.display_name(), "no-label");
        });
    }

    #[test]
    fn test_list_with_label() {
        with_temp_home(|_| {
            let sess = Session {
                name: "labeled-session".to_string(),
                label: Some("user/feature".to_string()),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                color: None,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].name, "labeled-session");
            assert_eq!(sessions[0].label.as_deref(), Some("user/feature"));
            assert_eq!(sessions[0].display_name(), "user/feature");
        });
    }
}
