use anyhow::{bail, Result};

/// Return the user's home directory from the HOME environment variable.
/// Returns an error if HOME is not set or is empty.
pub fn home_dir() -> Result<String> {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => Ok(h),
        _ => bail!("HOME environment variable is not set or is empty."),
    }
}

/// Return the box root directory. Uses `BOX_ROOT` if set, otherwise `$HOME/.box`.
pub fn box_root() -> Result<std::path::PathBuf> {
    if let Ok(root) = std::env::var("BOX_ROOT") {
        if !root.is_empty() {
            return Ok(std::path::PathBuf::from(root));
        }
    }
    Ok(std::path::PathBuf::from(home_dir()?).join(".box"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ENV_LOCK;

    #[test]
    fn test_box_root_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_home = std::env::var("HOME").ok();
        let saved_root = std::env::var("BOX_ROOT").ok();
        std::env::set_var("HOME", "/home/test");
        std::env::remove_var("BOX_ROOT");
        let result = box_root().unwrap();
        assert_eq!(result, std::path::PathBuf::from("/home/test/.box"));
        match saved_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match saved_root {
            Some(r) => std::env::set_var("BOX_ROOT", r),
            None => std::env::remove_var("BOX_ROOT"),
        }
    }

    #[test]
    fn test_box_root_override() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved_home = std::env::var("HOME").ok();
        let saved_root = std::env::var("BOX_ROOT").ok();
        std::env::set_var("HOME", "/home/test");
        std::env::set_var("BOX_ROOT", "/tmp/custom-box");
        let result = box_root().unwrap();
        assert_eq!(result, std::path::PathBuf::from("/tmp/custom-box"));
        match saved_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match saved_root {
            Some(r) => std::env::set_var("BOX_ROOT", r),
            None => std::env::remove_var("BOX_ROOT"),
        }
    }

    #[test]
    fn test_home_dir_returns_value() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("HOME").ok();
        std::env::set_var("HOME", "");
        let result = home_dir();
        assert!(result.is_err());
        match saved {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
