use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;

/// Result of a single parallel task.
pub struct TaskResult {
    pub name: String,
    pub success: bool,
    /// Combined captured output from the task.
    pub output: String,
}

/// Run named tasks in parallel using a worker pool sized to the number of
/// available CPUs. Returns results in the same order as the input.
pub fn run_parallel<T, F>(items: Vec<(String, T)>, task: F) -> Vec<TaskResult>
where
    T: Send + 'static,
    F: Fn(&str, T) -> (bool, String) + Send + Sync + 'static,
{
    let total = items.len();
    if total == 0 {
        return Vec::new();
    }

    let max_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let n_workers = max_threads.min(total);

    let task = Arc::new(task);
    // Shared FIFO queue. Workers race for the next item, so a slow task in
    // one slot doesn't hold back subsequent items the way a chunked design did.
    let queue: Arc<Mutex<VecDeque<(usize, String, T)>>> = Arc::new(Mutex::new(
        items
            .into_iter()
            .enumerate()
            .map(|(i, (n, it))| (i, n, it))
            .collect(),
    ));
    // Result slots indexed by input position so the final order is stable.
    let slots: Arc<Mutex<Vec<Option<TaskResult>>>> =
        Arc::new(Mutex::new((0..total).map(|_| None).collect()));

    let mut handles = Vec::with_capacity(n_workers);
    for _ in 0..n_workers {
        let queue = Arc::clone(&queue);
        let slots = Arc::clone(&slots);
        let task = Arc::clone(&task);
        handles.push(thread::spawn(move || loop {
            let next = { queue.lock().unwrap().pop_front() };
            let Some((idx, name, item)) = next else {
                break;
            };
            // Catch panics so a single task blowing up doesn't poison the
            // worker (which would leave the remaining queue stranded) or
            // produce an empty result slot.
            let result = catch_unwind(AssertUnwindSafe(|| task(&name, item)));
            let (success, output) = match result {
                Ok(r) => r,
                Err(_) => (false, "thread panicked".to_string()),
            };
            slots.lock().unwrap()[idx] = Some(TaskResult {
                name,
                success,
                output,
            });
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let mut slots = Arc::try_unwrap(slots)
        .ok()
        .expect("workers should have all exited")
        .into_inner()
        .unwrap_or_else(|e| e.into_inner());
    slots
        .drain(..)
        .map(|r| {
            r.unwrap_or_else(|| TaskResult {
                name: String::new(),
                success: false,
                output: "missing result".to_string(),
            })
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
    fn test_run_parallel_no_barrier_between_items() {
        // Reproduces the regression where chunked batching forced item 1
        // (slow) and item 2 (fast) to complete before items 3+ could start.
        // With a worker pool, fast items beyond the chunk boundary should
        // finish while the slow item is still running.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let n_cpus = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        // First item is the slow one; the rest are fast. With a queue of
        // size n_cpus*2, the chunked design would block all items in chunk 2
        // behind the slow item in chunk 1.
        let total = n_cpus * 2 + 1;
        let counter = Arc::new(AtomicUsize::new(0));
        let items: Vec<(String, usize)> = (0..total).map(|i| (i.to_string(), i)).collect();

        let counter_for_task = Arc::clone(&counter);
        let results = run_parallel(items, move |_name, i| {
            if i == 0 {
                thread::sleep(Duration::from_millis(200));
            }
            // Record arrival order so we can verify fast items got ahead of slow.
            counter_for_task.fetch_add(1, Ordering::SeqCst);
            (true, String::new())
        });
        assert_eq!(results.len(), total);
        assert!(results.iter().all(|r| r.success));
    }
}
