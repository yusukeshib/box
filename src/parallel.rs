use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

/// Result of a single parallel task.
pub struct TaskResult {
    pub name: String,
    pub success: bool,
    /// Combined captured output from the task.
    pub output: String,
}

/// Event emitted by `run_parallel_with_events` as tasks progress.
#[derive(Clone, Debug)]
pub enum ProgressEvent {
    Start(String),
    Finish(String, bool),
}

/// Run named tasks in parallel, capped at the number of available CPUs.
/// Returns results in the same order as the input.
pub fn run_parallel<T, F>(items: Vec<(String, T)>, task: F) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    run_parallel_inner(items, None, task)
}

/// Like `run_parallel`, but emits a `ProgressEvent::Start` before each task
/// and `ProgressEvent::Finish` after each completes. The channel is dropped
/// when all tasks are finished, so receivers can use the disconnect signal
/// to exit their render loops.
pub fn run_parallel_with_events<T, F>(
    items: Vec<(String, T)>,
    tx: Sender<ProgressEvent>,
    task: F,
) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    run_parallel_inner(items, Some(tx), task)
}

fn run_parallel_inner<T, F>(
    items: Vec<(String, T)>,
    tx: Option<Sender<ProgressEvent>>,
    task: F,
) -> Vec<TaskResult>
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
                let tx_clone = tx.clone();
                let handle = thread::spawn(move || {
                    if let Some(tx) = &tx_clone {
                        let _ = tx.send(ProgressEvent::Start(name_clone.clone()));
                    }
                    let (success, output) = task(&name_clone, item);
                    if let Some(tx) = &tx_clone {
                        let _ = tx.send(ProgressEvent::Finish(name_clone.clone(), success));
                    }
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
                Err(_) => {
                    if let Some(tx) = &tx {
                        let _ = tx.send(ProgressEvent::Finish(name.clone(), false));
                    }
                    TaskResult {
                        name,
                        success: false,
                        output: "thread panicked".to_string(),
                    }
                }
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
    use std::sync::mpsc;

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

    #[test]
    fn test_run_parallel_with_events_emits_start_and_finish() {
        let (tx, rx) = mpsc::channel();
        let items: Vec<(String, bool)> = vec![("a".into(), true), ("b".into(), false)];
        let results =
            run_parallel_with_events(items, tx, |_name, success| (success, String::new()));
        assert_eq!(results.len(), 2);

        let events: Vec<ProgressEvent> = rx.iter().collect();
        // Two starts and two finishes, one per item.
        let starts = events
            .iter()
            .filter(|e| matches!(e, ProgressEvent::Start(_)))
            .count();
        let finishes = events
            .iter()
            .filter(|e| matches!(e, ProgressEvent::Finish(_, _)))
            .count();
        assert_eq!(starts, 2);
        assert_eq!(finishes, 2);
        // One finish must be success=false (item "b").
        let b_failed = events
            .iter()
            .any(|e| matches!(e, ProgressEvent::Finish(n, false) if n == "b"));
        assert!(b_failed);
    }
}
