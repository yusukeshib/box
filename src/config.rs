use anyhow::{bail, Result};
use serde::Deserialize;

pub const DEFAULT_IMAGE: &str = "alpine:latest";

#[derive(Debug, Clone, PartialEq)]
pub enum Strategy {
    Clone,
    Worktree,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Strategy::Clone => write!(f, "clone"),
            Strategy::Worktree => write!(f, "worktree"),
        }
    }
}

impl std::str::FromStr for Strategy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "clone" => Ok(Strategy::Clone),
            "worktree" => Ok(Strategy::Worktree),
            _ => bail!("Invalid strategy '{}'. Must be 'clone' or 'worktree'.", s),
        }
    }
}

/// Return the user's home directory from the HOME environment variable.
/// Returns an error if HOME is not set or is empty.
pub fn home_dir() -> Result<String> {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => Ok(h),
        _ => bail!("HOME environment variable is not set or is empty."),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxConfig {
    pub name: String,
    pub project_dir: String,
    pub image: String,
    pub mount_path: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub local: bool,
    pub strategy: Strategy,
}

pub struct BoxConfigInput {
    pub name: String,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub project_dir: String,
    pub command: Option<Vec<String>>,
    pub env: Vec<String>,
    pub local: bool,
    pub strategy: Option<Strategy>,
}

fn resolve_command(command: Option<Vec<String>>) -> Result<Vec<String>> {
    match command {
        None => match std::env::var("BOX_DEFAULT_CMD") {
            Ok(val) if !val.is_empty() => shell_words::split(&val)
                .map_err(|e| anyhow::anyhow!("Failed to parse BOX_DEFAULT_CMD: {}", e)),
            _ => Ok(vec![]),
        },
        Some(cmd) => Ok(cmd),
    }
}

fn resolve_strategy(strategy: Option<Strategy>) -> Result<Strategy> {
    match strategy {
        Some(s) => Ok(s),
        None => {
            let env_val = std::env::var("BOX_STRATEGY").ok().filter(|v| !v.is_empty());
            match env_val {
                Some(s) => s.parse(),
                None => Ok(Strategy::Clone),
            }
        }
    }
}

pub fn resolve(input: BoxConfigInput) -> Result<BoxConfig> {
    let command = resolve_command(input.command)?;
    let strategy = resolve_strategy(input.strategy)?;

    if input.local {
        return Ok(BoxConfig {
            name: input.name,

            project_dir: input.project_dir,
            image: String::new(),
            mount_path: String::new(),
            command,
            env: vec![],
            local: true,
            strategy,
        });
    }

    let mount_path = input
        .mount_path
        .unwrap_or_else(|| derive_mount_path(&input.project_dir));
    let image = input.image.unwrap_or_else(|| {
        std::env::var("BOX_DEFAULT_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.to_string())
    });

    Ok(BoxConfig {
        name: input.name,
        project_dir: input.project_dir,
        image,
        mount_path,
        command,
        env: input.env,
        local: false,
        strategy,
    })
}

/// Default prefix key: Ctrl+P (0x10).
const DEFAULT_PREFIX_KEY: u8 = 0x10;

#[derive(Deserialize, Default)]
struct FileConfig {
    mux: Option<MuxFileConfig>,
}

#[derive(Deserialize, Default)]
struct MuxFileConfig {
    prefix_key: Option<String>,
}

/// Parse a prefix key string like "Ctrl+B" into its control byte (0x01..0x1A).
/// Returns `None` for invalid strings.
fn parse_prefix_key(s: &str) -> Option<u8> {
    let s = s.trim();
    let rest = s.strip_prefix("Ctrl+")?;
    if rest.len() != 1 {
        return None;
    }
    let ch = rest.chars().next()?.to_ascii_uppercase();
    if ch.is_ascii_uppercase() {
        Some(ch as u8 - b'A' + 1)
    } else {
        None
    }
}

/// Load the mux prefix key from `~/.config/box/config.toml`.
/// Returns the default (Ctrl+P = 0x10) if the file doesn't exist or the key
/// is not set / invalid.
pub fn load_mux_prefix_key() -> u8 {
    let home = match home_dir() {
        Ok(h) => h,
        Err(_) => return DEFAULT_PREFIX_KEY,
    };
    let path = std::path::Path::new(&home)
        .join(".config")
        .join("box")
        .join("config.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return DEFAULT_PREFIX_KEY,
    };
    let file_config: FileConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(_) => return DEFAULT_PREFIX_KEY,
    };
    file_config
        .mux
        .and_then(|m| m.prefix_key)
        .and_then(|s| parse_prefix_key(&s))
        .unwrap_or(DEFAULT_PREFIX_KEY)
}

pub fn derive_mount_path(project_dir: &str) -> String {
    let trimmed = project_dir.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/workspace".to_string();
    }
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => format!("/workspace/{}", name),
        _ => "/workspace".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate environment variables
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_derive_mount_path_normal() {
        assert_eq!(derive_mount_path("/home/user/myapp"), "/workspace/myapp");
    }

    #[test]
    fn test_derive_mount_path_nested() {
        assert_eq!(
            derive_mount_path("/home/user/projects/myapp"),
            "/workspace/myapp"
        );
    }

    #[test]
    fn test_derive_mount_path_root_fallback() {
        assert_eq!(derive_mount_path("/"), "/workspace");
    }

    #[test]
    fn test_derive_mount_path_trailing_slash() {
        assert_eq!(derive_mount_path("/home/user/myapp/"), "/workspace/myapp");
    }

    #[test]
    fn test_derive_mount_path_single_component() {
        assert_eq!(derive_mount_path("/myproject"), "/workspace/myproject");
    }

    #[test]
    fn test_resolve_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved_image = std::env::var("BOX_DEFAULT_IMAGE").ok();
        let saved_cmd = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::remove_var("BOX_DEFAULT_IMAGE");
        std::env::remove_var("BOX_DEFAULT_CMD");

        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();

        assert_eq!(
            config,
            BoxConfig {
                name: "test".to_string(),

                project_dir: "/home/user/myproject".to_string(),
                image: DEFAULT_IMAGE.to_string(),
                mount_path: "/workspace/myproject".to_string(),
                command: vec![],
                env: vec![],
                local: false,

                strategy: Strategy::Clone,
            }
        );

        if let Some(v) = saved_image {
            std::env::set_var("BOX_DEFAULT_IMAGE", v);
        }
        if let Some(v) = saved_cmd {
            std::env::set_var("BOX_DEFAULT_CMD", v);
        }
    }

    #[test]
    fn test_resolve_mount_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("BOX_DEFAULT_CMD");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: Some("/custom".to_string()),
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();

        assert_eq!(config.mount_path, "/custom");
    }

    #[test]
    fn test_resolve_image_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("BOX_DEFAULT_CMD");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: Some("ubuntu:latest".to_string()),
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();

        assert_eq!(config.image, "ubuntu:latest");
    }

    #[test]
    fn test_resolve_env_default_image() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved_cmd = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::remove_var("BOX_DEFAULT_CMD");
        std::env::set_var("BOX_DEFAULT_IMAGE", "ubuntu:latest");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.image, "ubuntu:latest");
        std::env::remove_var("BOX_DEFAULT_IMAGE");
        if let Some(v) = saved_cmd {
            std::env::set_var("BOX_DEFAULT_CMD", v);
        }
    }

    #[test]
    fn test_resolve_image_flag_overrides_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved_cmd = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::remove_var("BOX_DEFAULT_CMD");
        std::env::set_var("BOX_DEFAULT_IMAGE", "ubuntu:latest");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: Some("python:3.11".to_string()),
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.image, "python:3.11");
        std::env::remove_var("BOX_DEFAULT_IMAGE");
        if let Some(v) = saved_cmd {
            std::env::set_var("BOX_DEFAULT_CMD", v);
        }
    }

    #[test]
    fn test_home_dir_returns_value() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/test");
        let result = home_dir();
        assert_eq!(result.unwrap(), "/home/test");
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_home_dir_errors_when_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let result = home_dir();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("HOME"));
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_home_dir_errors_when_empty() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("HOME").ok();
        std::env::set_var("HOME", "");
        let result = home_dir();
        assert!(result.is_err());
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_resolve_full() {
        let _lock = ENV_LOCK.lock().unwrap();
        let config = resolve(BoxConfigInput {
            name: "full".to_string(),

            image: Some("python:3.11".to_string()),
            mount_path: Some("/app".to_string()),
            project_dir: "/home/user/project".to_string(),
            command: Some(vec!["python".to_string(), "main.py".to_string()]),
            env: vec!["FOO=bar".to_string()],
            local: false,

            strategy: None,
        })
        .unwrap();

        assert_eq!(
            config,
            BoxConfig {
                name: "full".to_string(),

                project_dir: "/home/user/project".to_string(),
                image: "python:3.11".to_string(),
                mount_path: "/app".to_string(),
                command: vec!["python".to_string(), "main.py".to_string()],
                env: vec!["FOO=bar".to_string()],
                local: false,

                strategy: Strategy::Clone,
            }
        );
    }

    #[test]
    fn test_resolve_env_default_cmd() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, vec!["bash".to_string()]);
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_cli_cmd_overrides_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: Some(vec!["sh".to_string()]),
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, vec!["sh".to_string()]);
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_env_default_cmd_multi_word() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash -c 'echo hello'");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(
            config.command,
            vec![
                "bash".to_string(),
                "-c".to_string(),
                "echo hello".to_string()
            ]
        );
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_env_default_cmd_empty() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, Vec::<String>::new());
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_env_default_cmd_invalid_parse() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash -c 'unclosed");
        let result = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("BOX_DEFAULT_CMD"));
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_env_default_cmd_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::remove_var("BOX_DEFAULT_CMD");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, Vec::<String>::new());
        if let Some(v) = saved {
            std::env::set_var("BOX_DEFAULT_CMD", v);
        }
    }

    #[test]
    fn test_resolve_local_respects_default_cmd() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            local: true,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, vec!["bash".to_string()]);
        assert!(config.local);
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }

    #[test]
    fn test_resolve_explicit_empty_command_skips_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),

            image: None,
            mount_path: None,
            project_dir: "/home/user/myproject".to_string(),
            command: Some(vec![]),
            env: vec![],
            local: false,

            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, Vec::<String>::new());
        match saved {
            Some(v) => std::env::set_var("BOX_DEFAULT_CMD", v),
            None => std::env::remove_var("BOX_DEFAULT_CMD"),
        }
    }
}
