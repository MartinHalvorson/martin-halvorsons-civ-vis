//! Running independent games at once.
//!
//! The engine exists to be simulated against: benchmarks, soaks, tournaments,
//! and self-play all play a batch of games that share nothing but the
//! ruleset, which is immutable. A batch is therefore embarrassingly parallel,
//! and running it on one core left most of the machine idle.
//!
//! Every game is still seeded and deterministic, and results are returned in
//! job order, so a batch produces exactly what it produced serially — only
//! sooner.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Stack a worker gets. Turn resolution nests deeply enough that a whole game
/// does not fit in a thread's stock 2 MiB, which is a quarter of the 8 MiB the
/// main thread hands the serial path: a batch that completed serially aborted
/// with a stack overflow as soon as it was spread across workers, and the
/// larger the armies grew the sooner it happened.
const WORKER_STACK: usize = 32 * 1024 * 1024;

/// Spawn one worker with a stack a whole game fits in.
fn spawn_worker<'scope, 'env, F>(scope: &'scope std::thread::Scope<'scope, 'env>, worker: F)
where
    F: FnOnce() + Send + 'scope,
{
    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn_scoped(scope, worker)
        .expect("the operating system refused a worker thread");
}

/// How many jobs to run at once by default: one per core.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(1)
}

/// Run `count` independent jobs across `jobs` threads, returning their
/// results in index order.
///
/// Jobs are handed out one at a time rather than in equal blocks, because
/// games of the same length are not games of the same cost — one that ends in
/// an early conquest is far cheaper than one that runs to the turn limit.
pub fn map<T, F>(count: usize, jobs: usize, job: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    let threads = jobs.clamp(1, count.max(1));
    if threads == 1 {
        return (0..count).map(job).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let slots = &slots;
    let job = &job;
    let next = &next;
    std::thread::scope(|scope| {
        for _ in 0..threads {
            spawn_worker(scope, move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= count {
                    break;
                }
                let value = job(index);
                *slots[index].lock().expect("a job panicked mid-write") = Some(value);
            });
        }
    });
    slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            slot.lock()
                .expect("a job panicked mid-write")
                .take()
                .unwrap_or_else(|| panic!("job {index} produced no result"))
        })
        .collect()
}

/// Like [`map`], but reports each result as soon as every job before it has
/// finished, so a long batch still prints in order as it goes.
pub fn map_reporting<T, F, R>(count: usize, jobs: usize, job: F, mut report: R) -> Vec<T>
where
    T: Send + Clone,
    F: Fn(usize) -> T + Sync,
    R: FnMut(usize, &T) + Send,
{
    let threads = jobs.clamp(1, count.max(1));
    if threads == 1 {
        return (0..count)
            .map(|index| {
                let value = job(index);
                report(index, &value);
                value
            })
            .collect();
    }
    let next = AtomicUsize::new(0);
    let done: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let reported = Mutex::new(0usize);
    let report = Mutex::new(&mut report);
    let (done, reported, report, next, job) = (&done, &reported, &report, &next, &job);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            spawn_worker(scope, move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= count {
                    break;
                }
                *done[index].lock().expect("a job panicked mid-write") = Some(job(index));
                // Flush whatever prefix is now complete. Holding the counter
                // while reporting keeps the order, and only one thread can be
                // inside this at a time.
                let mut cursor = reported.lock().expect("a job panicked mid-report");
                while *cursor < count {
                    let ready = done[*cursor].lock().expect("a job panicked mid-write");
                    let Some(value) = ready.as_ref() else { break };
                    report.lock().expect("a job panicked mid-report")(*cursor, value);
                    *cursor += 1;
                }
            });
        }
    });
    done.iter()
        .map(|slot| {
            slot.lock()
                .expect("a job panicked mid-write")
                .take()
                .expect("every job produced a result")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn results_come_back_in_job_order() {
        let squares = super::map(64, 8, |index| index * index);
        assert_eq!(squares, (0..64).map(|i| i * i).collect::<Vec<_>>());
    }

    #[test]
    fn one_thread_is_the_serial_path() {
        assert_eq!(super::map(5, 1, |index| index + 1), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn reports_in_order_however_jobs_finish() {
        let seen = std::sync::Mutex::new(Vec::new());
        let values = super::map_reporting(
            32,
            8,
            |index| {
                // Reverse the natural finishing order as far as the scheduler
                // allows, so in-order reporting is actually doing work.
                std::thread::sleep(std::time::Duration::from_micros((32 - index as u64) * 50));
                index
            },
            |index, value| seen.lock().unwrap().push((index, *value)),
        );
        assert_eq!(values, (0..32).collect::<Vec<_>>());
        assert_eq!(
            seen.into_inner().unwrap(),
            (0..32).map(|i| (i, i)).collect::<Vec<_>>()
        );
    }
}
