use anyhow::{bail, Result};

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
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub strategy: Strategy,
}

pub struct BoxConfigInput {
    pub name: String,
    pub project_dir: String,
    pub command: Option<Vec<String>>,
    pub env: Vec<String>,
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

    Ok(BoxConfig {
        name: input.name,
        project_dir: input.project_dir,
        command,
        env: input.env,
        strategy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate environment variables
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_resolve_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved_cmd = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::remove_var("BOX_DEFAULT_CMD");

        let config = resolve(BoxConfigInput {
            name: "test".to_string(),
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            strategy: None,
        })
        .unwrap();

        assert_eq!(
            config,
            BoxConfig {
                name: "test".to_string(),
                project_dir: "/home/user/myproject".to_string(),
                command: vec![],
                env: vec![],
                strategy: Strategy::Clone,
            }
        );

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
            project_dir: "/home/user/project".to_string(),
            command: Some(vec!["python".to_string(), "main.py".to_string()]),
            env: vec!["FOO=bar".to_string()],
            strategy: None,
        })
        .unwrap();

        assert_eq!(
            config,
            BoxConfig {
                name: "full".to_string(),
                project_dir: "/home/user/project".to_string(),
                command: vec!["python".to_string(), "main.py".to_string()],
                env: vec!["FOO=bar".to_string()],
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
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
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
            project_dir: "/home/user/myproject".to_string(),
            command: Some(vec!["sh".to_string()]),
            env: vec![],
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
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
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
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
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
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
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
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
            strategy: None,
        })
        .unwrap();
        assert_eq!(config.command, Vec::<String>::new());
        if let Some(v) = saved {
            std::env::set_var("BOX_DEFAULT_CMD", v);
        }
    }

    #[test]
    fn test_resolve_respects_default_cmd() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),
            project_dir: "/home/user/myproject".to_string(),
            command: None,
            env: vec![],
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
    fn test_resolve_explicit_empty_command_skips_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let saved = std::env::var("BOX_DEFAULT_CMD").ok();
        std::env::set_var("BOX_DEFAULT_CMD", "bash");
        let config = resolve(BoxConfigInput {
            name: "test".to_string(),
            project_dir: "/home/user/myproject".to_string(),
            command: Some(vec![]),
            env: vec![],
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
