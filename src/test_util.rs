use std::sync::Mutex;

/// Global lock for tests that mutate the HOME environment variable.
/// All test modules must use this single lock to avoid races.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());
