//! Executors for independent, pure preparation tasks (28, 38).
//!
//! A [`Workers`] runs a batch of tasks and returns once all of them have
//! finished. Tasks never touch the document or a session's memos: each one
//! writes its result into its own slot, and the caller reads the slots in index
//! order afterwards. Output therefore does not depend on the executor, its
//! thread count or the order tasks complete in. See `docs/incremental.md`
//! ("Native workers").

/// One task. It records its result in a slot it captured.
pub type Task<'a> = Box<dyn FnOnce() + Send + 'a>;

/// Runs a batch of independent tasks.
///
/// Implementations may run the tasks in any order, on any threads, but must
/// not return before every task they ran has finished. A task that is never
/// run only costs time: the caller computes its result itself.
pub trait Workers: Send + Sync {
    fn run<'a>(&self, tasks: Vec<Task<'a>>);
    /// How many tasks may run at once. Informational only: layout never reads
    /// it, so it cannot change how work is split or charged.
    fn threads(&self) -> usize {
        1
    }
}

/// Runs tasks one after another on the calling thread. The reference path and
/// WASM use it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Serial;

impl Workers for Serial {
    fn run<'a>(&self, tasks: Vec<Task<'a>>) {
        for task in tasks {
            task();
        }
    }
}

/// Runs tasks on `n` scoped threads, the calling thread included. Threads take
/// the next task from one shared queue, so completion order varies from run to
/// run. `Threads(0)` behaves like `Threads(1)`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Threads(pub usize);

#[cfg(not(target_arch = "wasm32"))]
impl Workers for Threads {
    fn run<'a>(&self, tasks: Vec<Task<'a>>) {
        let threads = self.0.max(1).min(tasks.len());
        if threads <= 1 {
            Serial.run(tasks);
            return;
        }
        let queue = std::sync::Mutex::new(tasks.into_iter());
        // A task runs outside the lock, so a poisoned lock still holds a
        // consistent queue.
        let next = || {
            queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next()
        };
        std::thread::scope(|scope| {
            for _ in 1..threads {
                scope.spawn(|| {
                    while let Some(task) = next() {
                        task();
                    }
                });
            }
            while let Some(task) = next() {
                task();
            }
        });
    }
    fn threads(&self) -> usize {
        self.0.max(1)
    }
}

/// Applies `f` to every input on `workers`, returning results in input order.
/// A result the executor did not produce is computed here, serially.
pub(crate) fn map<I: Sync, T: Send>(
    workers: &dyn Workers,
    inputs: &[I],
    f: &(dyn Fn(&I) -> T + Sync),
) -> Vec<T> {
    let mut slots: Vec<Option<T>> = inputs.iter().map(|_| None).collect();
    if inputs.len() > 1 {
        let tasks: Vec<Task<'_>> = slots
            .iter_mut()
            .zip(inputs)
            .map(|(slot, input)| Box::new(move || *slot = Some(f(input))) as Task<'_>)
            .collect();
        workers.run(tasks);
    }
    slots
        .into_iter()
        .zip(inputs)
        .map(|(slot, input)| slot.unwrap_or_else(|| f(input)))
        .collect()
}

/// Applies `f` to every item on `workers`. An item the executor skipped is
/// handled here, serially.
pub(crate) fn for_each_mut<I: Send>(
    workers: &dyn Workers,
    items: &mut [I],
    f: &(dyn Fn(&mut I) + Sync),
) {
    let mut done = vec![false; items.len()];
    if items.len() > 1 {
        let tasks: Vec<Task<'_>> = items
            .iter_mut()
            .zip(done.iter_mut())
            .map(|(item, flag)| {
                Box::new(move || {
                    f(item);
                    *flag = true;
                }) as Task<'_>
            })
            .collect();
        workers.run(tasks);
    }
    for (item, flag) in items.iter_mut().zip(done) {
        if !flag {
            f(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drops every other task, as a broken host executor might.
    struct Lossy;
    impl Workers for Lossy {
        fn run<'a>(&self, tasks: Vec<Task<'a>>) {
            for task in tasks.into_iter().step_by(2) {
                task();
            }
        }
    }

    #[test]
    fn map_keeps_input_order_for_every_executor() {
        let inputs: Vec<u64> = (0..257).collect();
        let square = |x: &u64| x.wrapping_mul(*x);
        let expected: Vec<u64> = inputs.iter().map(square).collect();
        #[allow(unused_mut)]
        let mut executors: Vec<&dyn Workers> = vec![&Serial, &Lossy];
        #[cfg(not(target_arch = "wasm32"))]
        executors.extend([
            &Threads(0) as &dyn Workers,
            &Threads(1),
            &Threads(2),
            &Threads(1000),
        ]);
        for workers in executors {
            assert_eq!(map(workers, &inputs, &square), expected);
            assert!(map(workers, &[] as &[u64], &square).is_empty());
            assert_eq!(map(workers, &[3u64], &square), [9]);
        }
    }
}
