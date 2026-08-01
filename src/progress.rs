use crate::parallel::{run_parallel, TaskResult};

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
    if items.is_empty() {
        return Vec::new();
    }

    if !verbose {
        eprint!("\x1b[2m{}…\x1b[0m ", label);
    }

    let results = run_parallel(items, task);

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
