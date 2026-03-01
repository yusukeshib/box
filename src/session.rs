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
    pub image: String,
    pub mount_path: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub local: bool,
    pub strategy: String,
}

impl From<config::BoxConfig> for Session {
    fn from(cfg: config::BoxConfig) -> Self {
        Session {
            name: cfg.name,
            project_dir: cfg.project_dir,
            image: cfg.image,
            mount_path: cfg.mount_path,
            command: cfg.command,
            env: cfg.env,
            local: cfg.local,
            strategy: cfg.strategy,
        }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct SessionSummary {
    pub name: String,
    pub project_dir: String,
    pub image: String,
    pub command: String,
    pub created_at: String,
    pub running: bool,
    pub local: bool,
    pub strategy: String,
}

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = PathBuf::from(config::home_dir()?)
        .join(".box")
        .join("sessions");
    Ok(dir)
}

const RESERVED_NAMES: &[&str] = &[
    "create", "resume", "remove", "stop", "exec", "upgrade", "path", "config", "list", "ls",
];

/// Parse a user-supplied name into (workspace, session).
/// `"my-feature"` → `("my-feature", "default")`
/// `"my-feature/server"` → `("my-feature", "server")`
pub fn parse_name(input: &str) -> (&str, &str) {
    match input.split_once('/') {
        Some((ws, sess)) => (ws, sess),
        None => (input, "default"),
    }
}

/// Return the full `workspace/session` form of a name.
pub fn full_name(input: &str) -> String {
    let (ws, sess) = parse_name(input);
    format!("{}/{}", ws, sess)
}

/// Return the workspace part of a full session name.
pub fn workspace_name(name: &str) -> &str {
    parse_name(name).0
}

fn validate_part(part: &str, label: &str) -> Result<()> {
    if part.is_empty() {
        bail!("{} name is required.", label);
    }
    if RESERVED_NAMES.contains(&part) {
        bail!(
            "'{}' is a reserved name and cannot be used as a session name.",
            part
        );
    }
    if !part
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "Invalid session name '{}'. Use only letters, digits, hyphens, and underscores.",
            part
        );
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Session name is required.");
    }
    let (ws, sess) = parse_name(name);
    validate_part(ws, "Workspace")?;
    // Only allow at most one '/' — reject "a/b/c"
    if name.matches('/').count() > 1 {
        bail!(
            "Invalid session name '{}'. At most one '/' is allowed (workspace/session).",
            name
        );
    }
    // If the user explicitly provided a session part, validate it too
    if name.contains('/') {
        validate_part(sess, "Session")?;
    }
    Ok(())
}

pub fn session_exists(name: &str) -> Result<bool> {
    let full = full_name(name);
    Ok(sessions_dir()?.join(&full).is_dir())
}

/// Check whether a workspace directory exists under sessions/.
pub fn workspace_exists(workspace: &str) -> Result<bool> {
    let ws_dir = sessions_dir()?.join(workspace);
    if !ws_dir.is_dir() {
        return Ok(false);
    }
    // Must contain at least one session subdirectory
    Ok(fs::read_dir(&ws_dir)?
        .filter_map(|e| e.ok())
        .any(|e| e.path().is_dir() && e.path().join("project_dir").exists()))
}

/// List all session names within a workspace (e.g. ["default", "server"]).
pub fn workspace_sessions(workspace: &str) -> Result<Vec<String>> {
    let ws_dir = sessions_dir()?.join(workspace);
    if !ws_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&ws_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.path().join("project_dir").exists())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    Ok(names)
}

/// If `project_dir` points inside `~/.box/workspaces/<ws>/`, follow the
/// workspace session chain to find the original (non-workspace) project directory.
/// Returns the original path unchanged when it is not inside a workspace.
pub fn resolve_original_project_dir(project_dir: &str) -> String {
    let home = match config::home_dir() {
        Ok(h) => h,
        Err(_) => return project_dir.to_string(),
    };
    let workspaces_dir = PathBuf::from(&home).join(".box").join("workspaces");
    let workspaces_dir = match fs::canonicalize(&workspaces_dir) {
        Ok(p) => p,
        Err(_) => return project_dir.to_string(),
    };

    let mut current = project_dir.to_string();
    for _ in 0..10 {
        let current_path = match fs::canonicalize(&current) {
            Ok(p) => p,
            Err(_) => break,
        };
        let rel = match current_path.strip_prefix(&workspaces_dir) {
            Ok(r) => r,
            Err(_) => break, // not inside workspaces — we're done
        };
        let ws_name = match rel.components().next() {
            Some(c) => c.as_os_str().to_string_lossy().to_string(),
            None => break,
        };
        // Load any session in this workspace to read its project_dir
        let sessions = match workspace_sessions(&ws_name) {
            Ok(s) => s,
            Err(_) => break,
        };
        let first = match sessions.first() {
            Some(s) => s,
            None => break,
        };
        let full = format!("{}/{}", ws_name, first);
        let dir = match sessions_dir() {
            Ok(d) => d.join(&full),
            Err(_) => break,
        };
        let pd = match fs::read_to_string(dir.join("project_dir")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => break,
        };
        if pd == current {
            break; // self-referencing — stop
        }
        current = pd;
    }
    current
}

pub fn save(session: &Session) -> Result<()> {
    let full = full_name(&session.name);
    let dir = sessions_dir()?.join(&full);
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
    fs::write(dir.join("strategy"), &session.strategy)?;
    Ok(())
}

pub fn load(name: &str) -> Result<Session> {
    let full = full_name(name);
    let dir = sessions_dir()?.join(&full);

    // Auto-migrate flat session on load
    if !dir.is_dir() {
        let ws = workspace_name(&full);
        let ws_dir = sessions_dir()?.join(ws);
        if ws_dir.join("project_dir").exists() {
            let _ = migrate_flat_session(ws);
        }
    }

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

    let strategy = fs::read_to_string(dir.join("strategy"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "clone".to_string());

    Ok(Session {
        name: full,
        project_dir,
        image,
        mount_path,
        command,
        env,
        local,
        strategy,
    })
}

/// Migrate a flat (old-format) session directory to workspace/default.
/// `sessions/<name>/project_dir` exists → move all files into `sessions/<name>/default/`.
fn migrate_flat_session(name: &str) -> Result<()> {
    let dir = sessions_dir()?.join(name);
    let default_dir = dir.join("default");
    fs::create_dir_all(&default_dir)?;
    #[cfg(unix)]
    fs::set_permissions(&default_dir, fs::Permissions::from_mode(0o700))?;

    // Move all files (not directories) from dir into default/
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let file_name = entry.file_name();
            fs::rename(&path, default_dir.join(&file_name))?;
        }
    }
    Ok(())
}

fn read_session_summary(session_path: &std::path::Path, name: String) -> SessionSummary {
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

    let local = fs::read_to_string(session_path.join("mode"))
        .map(|s| s.trim() == "local")
        .unwrap_or(false);

    let strategy = fs::read_to_string(session_path.join("strategy"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "clone".to_string());

    SessionSummary {
        name,
        project_dir,
        image,
        command,
        created_at,
        running: false,
        local,
        strategy,
    }
}

pub fn list() -> Result<Vec<SessionSummary>> {
    let dir = sessions_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let mut ws_entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    ws_entries.sort_by_key(|e| e.file_name());

    for ws_entry in ws_entries {
        let ws_name = ws_entry.file_name().to_string_lossy().to_string();
        let ws_path = ws_entry.path();

        // Check if this is a flat (old-format) session: project_dir exists directly
        if ws_path.join("project_dir").exists() {
            // Auto-migrate to workspace/default
            let _ = migrate_flat_session(&ws_name);
        }

        // Scan sub-directories for session entries
        let mut sub_entries: Vec<_> = fs::read_dir(&ws_path)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir() && e.path().join("project_dir").exists())
            .collect();
        sub_entries.sort_by_key(|e| e.file_name());

        for sub_entry in sub_entries {
            let sess_name = sub_entry.file_name().to_string_lossy().to_string();
            let full_name = format!("{}/{}", ws_name, sess_name);
            let session_path = sub_entry.path();
            sessions.push(read_session_summary(&session_path, full_name));
        }
    }

    Ok(sessions)
}

pub fn remove_dir(name: &str) -> Result<()> {
    let full = full_name(name);
    let dir = sessions_dir()?.join(&full);
    fs::remove_dir_all(&dir).context(format!("Failed to remove session directory for '{}'", name))
}

/// Remove the entire workspace directory (all sessions within it).
pub fn remove_workspace_dir(workspace: &str) -> Result<()> {
    let dir = sessions_dir()?.join(workspace);
    fs::remove_dir_all(&dir).context(format!(
        "Failed to remove workspace directory for '{}'",
        workspace
    ))
}

pub fn touch_resumed_at(name: &str) -> Result<()> {
    let full = full_name(name);
    let dir = sessions_dir()?.join(&full);
    fs::write(
        dir.join("resumed_at"),
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )?;
    Ok(())
}

pub fn write_pid(name: &str, pid: u32) -> Result<()> {
    let full = full_name(name);
    let dir = sessions_dir()?.join(&full);
    fs::write(dir.join("pid"), pid.to_string())?;
    Ok(())
}

pub fn remove_pid(name: &str) {
    let full = full_name(name);
    if let Ok(dir) = sessions_dir() {
        let _ = fs::remove_file(dir.join(&full).join("pid"));
    }
}

pub fn socket_path(name: &str) -> Result<PathBuf> {
    let full = full_name(name);
    Ok(sessions_dir()?.join(&full).join("sock"))
}

pub fn remove_socket(name: &str) {
    let full = full_name(name);
    if let Ok(dir) = sessions_dir() {
        let _ = fs::remove_file(dir.join(&full).join("sock"));
    }
}

pub fn is_local_running(name: &str) -> bool {
    let full = full_name(name);
    // Check socket first (authoritative if server is up)
    if let Ok(path) = socket_path(&full) {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return true;
        }
    }
    // Fallback: PID check (handles server startup race)
    let dir = match sessions_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let pid_str = match fs::read_to_string(dir.join(&full).join("pid")) {
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
    fn test_parse_name_bare() {
        assert_eq!(parse_name("my-feature"), ("my-feature", "default"));
    }

    #[test]
    fn test_parse_name_with_slash() {
        assert_eq!(parse_name("my-feature/server"), ("my-feature", "server"));
    }

    #[test]
    fn test_full_name_bare() {
        assert_eq!(full_name("my-feature"), "my-feature/default");
    }

    #[test]
    fn test_full_name_with_slash() {
        assert_eq!(full_name("my-feature/server"), "my-feature/server");
    }

    #[test]
    fn test_workspace_name_bare() {
        assert_eq!(workspace_name("my-feature"), "my-feature");
    }

    #[test]
    fn test_workspace_name_with_slash() {
        assert_eq!(workspace_name("my-feature/server"), "my-feature");
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
        assert!(validate_name("my-feature/server").is_ok());
        assert!(validate_name("ws/default").is_ok());
    }

    #[test]
    fn test_validate_name_double_slash() {
        let err = validate_name("a/b/c").unwrap_err();
        assert!(err.to_string().contains("At most one '/'"));
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
                name: "test-ws/default".to_string(),
                project_dir: "/tmp/myproject".to_string(),
                image: "ubuntu:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("test-ws/default").unwrap();
            assert_eq!(loaded.name, "test-ws/default");
            assert_eq!(loaded.project_dir, "/tmp/myproject");
            assert_eq!(loaded.image, "ubuntu:latest");
            assert_eq!(loaded.mount_path, "/workspace");
            assert!(loaded.command.is_empty());
        });
    }

    #[test]
    fn test_save_and_load_bare_name_resolves_to_default() {
        with_temp_home(|_| {
            let sess = Session {
                name: "test-ws".to_string(),
                project_dir: "/tmp/myproject".to_string(),
                image: "ubuntu:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            // Loading with bare name should resolve to workspace/default
            let loaded = load("test-ws").unwrap();
            assert_eq!(loaded.name, "test-ws/default");
        });
    }

    #[test]
    fn test_save_and_load_with_command() {
        with_temp_home(|_| {
            let sess = Session {
                name: "full-ws/default".to_string(),
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

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("full-ws/default").unwrap();
            assert_eq!(loaded.command, vec!["bash", "-c", "echo hello"]);
        });
    }

    #[test]
    fn test_save_creates_metadata_files() {
        with_temp_home(|_| {
            let sess = Session {
                name: "meta-test/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("meta-test/default");
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
            let dir = sessions_dir().unwrap().join("broken/default");
            fs::create_dir_all(&dir).unwrap();
            // Don't write project_dir file

            let err = load("broken/default").unwrap_err();
            assert!(err
                .to_string()
                .contains("missing project directory metadata"));
        });
    }

    #[test]
    fn test_load_defaults_when_optional_files_missing() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("minimal/default");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "/tmp/project").unwrap();
            // Don't write image or mount_path

            let loaded = load("minimal/default").unwrap();
            assert_eq!(loaded.image, config::DEFAULT_IMAGE);
            assert_eq!(loaded.mount_path, config::derive_mount_path("/tmp/project"));
        });
    }

    #[test]
    fn test_session_exists() {
        with_temp_home(|_| {
            assert!(!session_exists("nope").unwrap());

            let sess = Session {
                name: "exists-test/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();
            assert!(session_exists("exists-test/default").unwrap());
            // Bare name should also find via default
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
            for name in &["alpha/default", "beta/default", "gamma/default"] {
                let sess = Session {
                    name: name.to_string(),
                    project_dir: format!("/tmp/{}", name.split('/').next().unwrap()),
                    image: "alpine:latest".to_string(),
                    mount_path: "/workspace".to_string(),
                    command: vec![],
                    env: vec![],
                    local: false,

                    strategy: "clone".to_string(),
                };
                save(&sess).unwrap();
            }

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 3);
            // Should be sorted alphabetically by full name
            assert_eq!(sessions[0].name, "alpha/default");
            assert_eq!(sessions[1].name, "beta/default");
            assert_eq!(sessions[2].name, "gamma/default");
        });
    }

    #[test]
    fn test_list_multiple_sessions_same_workspace() {
        with_temp_home(|_| {
            for sess_name in &["ws/default", "ws/server", "ws/test"] {
                let sess = Session {
                    name: sess_name.to_string(),
                    project_dir: "/tmp/project".to_string(),
                    image: "alpine:latest".to_string(),
                    mount_path: "/workspace".to_string(),
                    command: vec![],
                    env: vec![],
                    local: false,

                    strategy: "clone".to_string(),
                };
                save(&sess).unwrap();
            }

            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 3);
            assert_eq!(sessions[0].name, "ws/default");
            assert_eq!(sessions[1].name, "ws/server");
            assert_eq!(sessions[2].name, "ws/test");
        });
    }

    #[test]
    fn test_list_reads_metadata() {
        with_temp_home(|_| {
            let sess = Session {
                name: "list-meta/default".to_string(),
                project_dir: "/home/user/project".to_string(),
                image: "ubuntu:22.04".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

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
                name: "to-remove/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();
            assert!(session_exists("to-remove/default").unwrap());

            remove_dir("to-remove/default").unwrap();
            assert!(!session_exists("to-remove/default").unwrap());
        });
    }

    #[test]
    fn test_remove_dir_nonexistent() {
        with_temp_home(|_| {
            let err = remove_dir("nonexistent/default").unwrap_err();
            assert!(err.to_string().contains("Failed to remove"));
        });
    }

    #[test]
    fn test_touch_resumed_at() {
        with_temp_home(|_| {
            let sess = Session {
                name: "resume-test/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            touch_resumed_at("resume-test/default").unwrap();

            let dir = sessions_dir().unwrap().join("resume-test/default");
            let content = fs::read_to_string(dir.join("resumed_at")).unwrap();
            assert!(content.ends_with("UTC"));
        });
    }

    #[test]
    fn test_save_trims_whitespace_on_load() {
        with_temp_home(|_| {
            let dir = sessions_dir().unwrap().join("trim-test/default");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "  /tmp/project  \n").unwrap();
            fs::write(dir.join("image"), " ubuntu:latest \n").unwrap();
            fs::write(dir.join("mount_path"), " /src \n").unwrap();

            let loaded = load("trim-test/default").unwrap();
            assert_eq!(loaded.project_dir, "/tmp/project");
            assert_eq!(loaded.image, "ubuntu:latest");
            assert_eq!(loaded.mount_path, "/src");
        });
    }

    #[test]
    fn test_command_save_format() {
        with_temp_home(|_| {
            let sess = Session {
                name: "cmd-format/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec!["bash".to_string(), "-c".to_string(), "echo hi".to_string()],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("cmd-format/default");
            let raw = fs::read_to_string(dir.join("command")).unwrap();
            assert_eq!(raw, "bash\0-c\0echo hi");
        });
    }

    #[test]
    fn test_save_and_load_with_env() {
        with_temp_home(|_| {
            let sess = Session {
                name: "env-test/default".to_string(),
                project_dir: "/tmp/project".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec!["FOO=bar".to_string(), "BAZ".to_string()],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let loaded = load("env-test/default").unwrap();
            assert_eq!(loaded.env, vec!["FOO=bar", "BAZ"]);

            let dir = sessions_dir().unwrap().join("env-test/default");
            let raw = fs::read_to_string(dir.join("env")).unwrap();
            assert_eq!(raw, "FOO=bar\0BAZ");
        });
    }

    #[test]
    fn test_save_and_load_empty_env() {
        with_temp_home(|_| {
            let sess = Session {
                name: "no-env/default".to_string(),
                project_dir: "/tmp/project".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let dir = sessions_dir().unwrap().join("no-env/default");
            assert!(!dir.join("env").exists());

            let loaded = load("no-env/default").unwrap();
            assert!(loaded.env.is_empty());
        });
    }

    #[test]
    fn test_migration_flat_to_nested() {
        with_temp_home(|_| {
            // Create a flat (old-format) session manually
            let dir = sessions_dir().unwrap().join("old-session");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "/tmp/project").unwrap();
            fs::write(dir.join("image"), "alpine:latest").unwrap();
            fs::write(dir.join("mode"), "local").unwrap();
            fs::write(dir.join("strategy"), "clone").unwrap();

            // list() should auto-migrate
            let sessions = list().unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].name, "old-session/default");

            // The old flat file should be moved to default/
            assert!(!dir.join("project_dir").exists());
            assert!(dir.join("default").join("project_dir").exists());
        });
    }

    #[test]
    fn test_migration_on_load() {
        with_temp_home(|_| {
            // Create a flat (old-format) session manually
            let dir = sessions_dir().unwrap().join("old-load");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("project_dir"), "/tmp/project").unwrap();
            fs::write(dir.join("image"), "alpine:latest").unwrap();
            fs::write(dir.join("mode"), "local").unwrap();
            fs::write(dir.join("strategy"), "clone").unwrap();

            // load with bare name should auto-migrate and succeed
            let loaded = load("old-load").unwrap();
            assert_eq!(loaded.name, "old-load/default");
            assert_eq!(loaded.project_dir, "/tmp/project");
        });
    }

    #[test]
    fn test_workspace_exists() {
        with_temp_home(|_| {
            assert!(!workspace_exists("nope").unwrap());

            let sess = Session {
                name: "ws-test/default".to_string(),
                project_dir: "/tmp/p".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();
            assert!(workspace_exists("ws-test").unwrap());
        });
    }

    #[test]
    fn test_workspace_sessions() {
        with_temp_home(|_| {
            for name in &["ws/default", "ws/server", "ws/test"] {
                let sess = Session {
                    name: name.to_string(),
                    project_dir: "/tmp/p".to_string(),
                    image: "alpine:latest".to_string(),
                    mount_path: "/workspace".to_string(),
                    command: vec![],
                    env: vec![],
                    local: false,

                    strategy: "clone".to_string(),
                };
                save(&sess).unwrap();
            }

            let names = workspace_sessions("ws").unwrap();
            assert_eq!(names, vec!["default", "server", "test"]);
        });
    }

    #[test]
    fn test_resolve_original_non_workspace_passthrough() {
        with_temp_home(|_| {
            let result = resolve_original_project_dir("/tmp/my-real-repo");
            assert_eq!(result, "/tmp/my-real-repo");
        });
    }

    #[test]
    fn test_resolve_original_single_hop() {
        with_temp_home(|tmp| {
            // Create a workspace directory that looks like a real workspace
            let ws_dir = tmp.join(".box").join("workspaces").join("ws-a");
            fs::create_dir_all(&ws_dir).unwrap();

            // Create a session for ws-a pointing to the real repo
            let sess = Session {
                name: "ws-a/default".to_string(),
                project_dir: "/tmp/real-repo".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            let result = resolve_original_project_dir(&ws_dir.to_string_lossy());
            assert_eq!(result, "/tmp/real-repo");
        });
    }

    #[test]
    fn test_resolve_original_chained() {
        with_temp_home(|tmp| {
            // ws-b workspace dir points to ws-a workspace dir
            let ws_a_dir = tmp.join(".box").join("workspaces").join("ws-a");
            let ws_b_dir = tmp.join(".box").join("workspaces").join("ws-b");
            fs::create_dir_all(&ws_a_dir).unwrap();
            fs::create_dir_all(&ws_b_dir).unwrap();

            // ws-a session points to the real repo
            let sess_a = Session {
                name: "ws-a/default".to_string(),
                project_dir: "/tmp/real-repo".to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                strategy: "clone".to_string(),
            };
            save(&sess_a).unwrap();

            // ws-b session points to ws-a workspace dir
            let sess_b = Session {
                name: "ws-b/default".to_string(),
                project_dir: ws_a_dir.to_string_lossy().to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                strategy: "clone".to_string(),
            };
            save(&sess_b).unwrap();

            let result = resolve_original_project_dir(&ws_b_dir.to_string_lossy());
            assert_eq!(result, "/tmp/real-repo");
        });
    }

    #[test]
    fn test_resolve_original_missing_workspace() {
        with_temp_home(|tmp| {
            // Create the workspaces parent dir but not the specific workspace session
            let ws_dir = tmp.join(".box").join("workspaces").join("ghost");
            fs::create_dir_all(&ws_dir).unwrap();

            // No session metadata exists for "ghost" — should fall back gracefully
            let input = ws_dir.to_string_lossy().to_string();
            let result = resolve_original_project_dir(&input);
            assert_eq!(result, input);
        });
    }

    #[test]
    fn test_resolve_original_self_referencing() {
        with_temp_home(|tmp| {
            let ws_dir = tmp.join(".box").join("workspaces").join("loop-ws");
            fs::create_dir_all(&ws_dir).unwrap();

            // Session points to its own workspace dir (self-referencing)
            let sess = Session {
                name: "loop-ws/default".to_string(),
                project_dir: ws_dir.to_string_lossy().to_string(),
                image: "alpine:latest".to_string(),
                mount_path: "/workspace".to_string(),
                command: vec![],
                env: vec![],
                local: false,
                strategy: "clone".to_string(),
            };
            save(&sess).unwrap();

            // Should not infinite loop — returns the self-referencing path
            let input = ws_dir.to_string_lossy().to_string();
            let result = resolve_original_project_dir(&input);
            assert_eq!(result, input);
        });
    }
}
