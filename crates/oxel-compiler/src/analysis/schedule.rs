//! Schedules independent runtime body checks in declaration order.

use std::{sync::OnceLock, thread};

use rayon::{ThreadPool, ThreadPoolBuilder, prelude::*};

const MIN_BODIES: usize = 128;
const MIN_BODY_BYTES: usize = 64 * 1024;
const MIN_BATCH: usize = 16;
const DEFAULT_MAX_JOBS: usize = 8;

static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();

pub(crate) fn ordered<T: Sync, R: Send>(
    items: &[T],
    body_bytes: impl Fn(&T) -> usize,
    check: impl Fn(usize, &T) -> R + Sync + Send,
    mut apply: impl FnMut(R),
) {
    if items.len() >= MIN_BODIES
        && items.iter().map(body_bytes).sum::<usize>() >= MIN_BODY_BYTES
        && let Some(pool) = POOL.get_or_init(build_pool)
    {
        // Indexed collection preserves input order even if tasks finish out of order.
        let checked: Vec<_> = pool.install(|| {
            items
                .par_iter()
                .enumerate()
                .with_min_len(MIN_BATCH)
                .map(|(index, item)| check(index, item))
                .collect()
        });
        checked.into_iter().for_each(apply);
    } else {
        for (index, item) in items.iter().enumerate() {
            apply(check(index, item));
        }
    }
}

fn build_pool() -> Option<ThreadPool> {
    let available = thread::available_parallelism().map_or(1, usize::from);
    let jobs = std::env::var("OXEL_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<std::num::NonZeroUsize>().ok())
        .map_or(available.min(DEFAULT_MAX_JOBS), |jobs| jobs.get());
    if jobs == 1 {
        return None;
    }
    ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("oxel-check-{index}"))
        .stack_size(4 * 1024 * 1024)
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_checks_run_locally_and_worker_results_keep_input_order() {
        let items: Vec<_> = (0..MIN_BODIES).collect();
        let caller = thread::current().id();
        ordered(
            &items,
            |_| 1,
            |_, _| thread::current().id(),
            |thread| assert_eq!(thread, caller),
        );
        let mut results = Vec::new();
        ordered(
            &items,
            |_| MIN_BODY_BYTES / MIN_BODIES,
            |index, &item| (index, item, thread::current().id()),
            |result| results.push(result),
        );
        let parallel = POOL.get().unwrap().is_some();
        assert_eq!(results.len(), items.len());
        for (expected, (index, item, thread)) in results.into_iter().enumerate() {
            assert_eq!((index, item), (expected, expected));
            assert_eq!(thread != caller, parallel);
        }
    }
}
