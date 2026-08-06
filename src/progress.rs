use crate::parallel::{run_parallel_with_limit, TaskResult, DEFAULT_MAX_WORKERS};

/// Run tasks in parallel with a simple single-line status message.
/// Verbose mode leaves status reporting to the caller.
pub fn run_parallel_with_progress<T, F>(
    label: &str,
    items: Vec<(String, T)>,
    verbose: bool,
    task: F,
) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    run_parallel_with_progress_limit(label, items, verbose, DEFAULT_MAX_WORKERS, task)
}

/// Run tasks with progress output and a workload-specific worker limit.
pub fn run_parallel_with_progress_limit<T, F>(
    label: &str,
    items: Vec<(String, T)>,
    verbose: bool,
    max_workers: usize,
    task: F,
) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    if items.is_empty() {
        return Vec::new();
    }

    if !verbose {
        eprint!("\x1b[2m{}…\x1b[0m ", label);
    }

    let results = run_parallel_with_limit(items, max_workers, task);

    if !verbose {
        let failures = results.iter().filter(|result| !result.success).count();
        if failures == 0 {
            eprintln!("\x1b[32mok\x1b[0m");
        } else {
            eprintln!("\x1b[31m{} failed\x1b[0m", failures);
        }
    }

    results
}
