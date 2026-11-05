//! Shared CPU work scheduler for independent compiler queries.

use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

const MIN_PARALLEL_WORK: usize = 2 * 1024 * 1024;
const MAX_WORKERS: usize = 8;

struct Scheduler {
    sender: mpsc::Sender<Job>,
    workers: usize,
}

impl Scheduler {
    fn global() -> &'static Self {
        static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();
        SCHEDULER.get_or_init(|| {
            let workers = available_workers();
            let (sender, receiver) = mpsc::channel::<Job>();
            let receiver = Arc::new(Mutex::new(receiver));
            for index in 0..workers {
                let receiver = Arc::clone(&receiver);
                thread::Builder::new()
                    .name(format!("klf-query-{index}"))
                    .spawn(move || {
                        loop {
                            let job = receiver
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .recv();
                            match job {
                                Ok(job) => job(),
                                Err(_) => break,
                            }
                        }
                    })
                    .expect("start KLF query worker");
            }
            Self { sender, workers }
        })
    }
}

fn available_workers() -> usize {
    thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_WORKERS)
}

/// Maps independent compiler inputs in source order, batching small inputs
/// and scheduling only when the estimated work can amortize queue overhead.
/// The worker pool is shared by all sessions in this process.
pub fn parallel_map<I, O, F>(items: Vec<I>, estimated_work: usize, map: F) -> Vec<O>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    if items.len() < 2 || estimated_work < MIN_PARALLEL_WORK || available_workers() < 2 {
        return items.into_iter().map(map).collect();
    }

    let scheduler = Scheduler::global();
    if scheduler.workers < 2 {
        return items.into_iter().map(map).collect();
    }

    let count = items.len();
    let batch_size = adaptive_batch_size(count, estimated_work, scheduler.workers);
    let mut batches = Vec::with_capacity(count.div_ceil(batch_size));
    let mut batch = Vec::with_capacity(batch_size);
    for (index, item) in items.into_iter().enumerate() {
        batch.push((index, item));
        if batch.len() == batch_size {
            batches.push(std::mem::replace(
                &mut batch,
                Vec::with_capacity(batch_size),
            ));
        }
    }
    if !batch.is_empty() {
        batches.push(batch);
    }

    let map = Arc::new(map);
    let (sender, receiver) = mpsc::channel();
    for batch in batches {
        let sender = sender.clone();
        let map = Arc::clone(&map);
        scheduler
            .sender
            .send(Box::new(move || {
                let results = batch
                    .into_iter()
                    .map(|(index, item)| (index, map(item)))
                    .collect::<Vec<_>>();
                let _ = sender.send(results);
            }))
            .expect("KLF query scheduler is available");
    }
    drop(sender);

    let mut results: Vec<Option<O>> = std::iter::repeat_with(|| None).take(count).collect();
    let mut received = 0;
    while received < count {
        let batch = receiver
            .recv()
            .expect("KLF query worker returned a result batch");
        received += batch.len();
        for (index, result) in batch {
            results[index] = Some(result);
        }
    }
    results
        .into_iter()
        .map(|result| result.expect("every KLF query input has a result"))
        .collect()
}

fn adaptive_batch_size(count: usize, estimated_work: usize, workers: usize) -> usize {
    let average_work = estimated_work.div_ceil(count).max(1);
    let target_batch_work = (estimated_work / workers.saturating_mul(4).max(1)).max(4 * 1024);
    (target_batch_work / average_work).clamp(1, 64)
}

#[cfg(test)]
mod tests {
    use super::{adaptive_batch_size, parallel_map};

    #[test]
    fn tiny_inputs_are_batched_into_useful_tasks() {
        let batch = adaptive_batch_size(10_000, 1_000_000, 8);
        assert!(batch > 1);
        assert!(batch <= 64);
    }

    #[test]
    fn expensive_inputs_remain_independent_tasks() {
        assert_eq!(adaptive_batch_size(12, 1_200_000, 8), 1);
    }

    #[test]
    fn parallel_results_keep_source_order() {
        let input = (0..4096).collect::<Vec<_>>();
        let output = parallel_map(input.clone(), 4_000_000, |value| value * 2);
        assert_eq!(
            output,
            input.into_iter().map(|value| value * 2).collect::<Vec<_>>()
        );
    }
}
