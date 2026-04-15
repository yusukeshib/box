use std::sync::Arc;
use std::thread;

/// Result of a single parallel task.
pub struct TaskResult {
    pub name: String,
    pub success: bool,
    /// Combined captured output from the task.
    pub output: String,
}

/// Run named tasks in parallel, capped at the number of available CPUs.
/// Returns results in the same order as the input.
pub fn run_parallel<T, F>(items: Vec<(String, T)>, task: F) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    let max_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let task = Arc::new(task);
    let mut results = Vec::with_capacity(items.len());

    for chunk in items.chunks_vec(max_threads) {
        let handles: Vec<(String, thread::JoinHandle<TaskResult>)> = chunk
            .into_iter()
            .map(|(name, item)| {
                let task = Arc::clone(&task);
                let name_clone = name.clone();
                let handle = thread::spawn(move || {
                    let (success, output) = task(&name_clone, item);
                    TaskResult {
                        name: name_clone,
                        success,
                        output,
                    }
                });
                (name, handle)
            })
            .collect();

        for (name, handle) in handles {
            results.push(match handle.join() {
                Ok(result) => result,
                Err(_) => TaskResult {
                    name,
                    success: false,
                    output: "thread panicked".to_string(),
                },
            });
        }
    }

    results
}

/// Extension trait to chunk a Vec by ownership (avoids slice borrowing issues).
trait ChunksVec<T> {
    fn chunks_vec(self, size: usize) -> Vec<Vec<T>>;
}

impl<T> ChunksVec<T> for Vec<T> {
    fn chunks_vec(self, size: usize) -> Vec<Vec<T>> {
        let mut result = Vec::new();
        let mut iter = self.into_iter().peekable();
        while iter.peek().is_some() {
            result.push(iter.by_ref().take(size).collect());
        }
        result
    }
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

    #[test]
    fn test_run_parallel_preserves_name_on_panic() {
        let items: Vec<(String, bool)> = vec![("good".into(), false), ("panicker".into(), true)];
        let results = run_parallel(items, |_name, should_panic| {
            if should_panic {
                panic!("boom");
            }
            (true, String::new())
        });
        assert_eq!(results[1].name, "panicker");
        assert!(!results[1].success);
    }
}
