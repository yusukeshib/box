use std::sync::Arc;
use std::thread;

/// Result of a single parallel task.
pub struct TaskResult {
    pub name: String,
    pub success: bool,
    /// Combined captured output from the task.
    pub output: String,
}

/// Run named tasks in parallel (one std::thread per item).
/// Returns results in the same order as the input.
pub fn run_parallel<T, F>(items: Vec<(String, T)>, task: F) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    let task = Arc::new(task);

    let handles: Vec<_> = items
        .into_iter()
        .map(|(name, item)| {
            let task = Arc::clone(&task);
            let name_clone = name.clone();
            thread::spawn(move || {
                let (success, output) = task(&name_clone, item);
                TaskResult {
                    name: name_clone,
                    success,
                    output,
                }
            })
        })
        .collect();

    handles
        .into_iter()
        .map(|h| match h.join() {
            Ok(result) => result,
            Err(_) => TaskResult {
                name: String::from("unknown"),
                success: false,
                output: "thread panicked".to_string(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_parallel_collects_results_in_order() {
        let items: Vec<(String, u32)> = vec![("a".into(), 1), ("b".into(), 2), ("c".into(), 3)];
        let results = run_parallel(items, |name, val| (true, format!("{}={}", name, val)));
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].name, "a");
        assert_eq!(results[0].output, "a=1");
        assert_eq!(results[1].name, "b");
        assert_eq!(results[1].output, "b=2");
        assert_eq!(results[2].name, "c");
        assert_eq!(results[2].output, "c=3");
        assert!(results.iter().all(|r| r.success));
    }

    #[test]
    fn test_run_parallel_handles_failures() {
        let items: Vec<(String, bool)> = vec![("ok".into(), true), ("fail".into(), false)];
        let results = run_parallel(items, |_name, should_succeed| {
            (should_succeed, String::new())
        });
        assert!(results[0].success);
        assert!(!results[1].success);
    }
}
