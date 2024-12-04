//! An simple scoped thread pool implementation made in Rust.
//!
//! # Examples
//! ```rust
//! use threadpool::ThreadPool;
//!
//! let tp = ThreadPool::default();
//!
//! let mut my_vec = vec![0, 0, 0, 0, 0, 0, 0, 0, 0];
//!
//! tp.with_scope(|scope| {
//!     for value in &mut my_vec {
//!         scope.enqueue_task(|| *value += 10);
//!     }
//! });
//!
//! assert_eq!(my_vec, vec![10, 10, 10, 10, 10, 10, 10, 10, 10]);
//! ```
use std::marker::PhantomData;
use std::panic;
use std::sync::atomic::{self, AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

type Task<'task> = Box<dyn FnOnce() + Send + 'task>;

pub struct ThreadPool {
    sender: Sender<Task<'static>>,
    state: Arc<ThreadPoolState>,
}

struct ThreadPoolState {
    tasks: AtomicU64,
    condvar: Condvar,
    finished_mutex: Mutex<bool>,
    panicked: AtomicBool,
}

impl Default for ThreadPool {
    fn default() -> Self {
        ThreadPool::new(std::thread::available_parallelism().unwrap().get())
    }
}

impl ThreadPool {
    /// Create a ThreadPool with 'size' workers.
    ///
    /// # Panic
    /// Panics if the user attemps to create it with 0 workers.
    pub fn new(size: usize) -> Self {
        assert!(size > 0);

        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));

        let state = Arc::new(ThreadPoolState {
            tasks: AtomicU64::new(1),
            condvar: Condvar::new(),
            finished_mutex: Mutex::new(false),
            panicked: AtomicBool::new(false),
        });

        for _ in 0..size {
            ThreadPool::spawn_worker(receiver.clone(), state.clone());
        }

        ThreadPool { sender, state }
    }

    /// Create a scope where it's possible to send tasks to threads without 'static lifetime.
    ///
    /// # Panic
    /// Will panic if any thread panics while executing the task.
    pub fn with_scope<'pool, F>(&'pool self, f: F)
    where
        F: for<'scope> FnOnce(&'scope Scope<'scope, 'pool>),
    {
        let scope = Scope {
            threadpool: self,
            scope: PhantomData,
        };

        f(&scope);

        // Verify if all the tasks from the scope has finished
        // NOTE: The task counter started with '1' to avoid a race condition where the tasks reach
        //       '0' before all tasks were spawned. By considering the scope building as a task
        //       itself we ensure that all tasks were spawned and executed before dropping the
        //       scope.
        let mut finished = self.state.finished_mutex.lock().unwrap();
        if self.state.tasks.fetch_sub(1, Ordering::Release) > 1 {
            while !(*finished) {
                finished = self.state.condvar.wait(finished).unwrap();
            }
        }

        if self.state.panicked.load(Ordering::Acquire) {
            panic!("One thread in the threadpool panicked!")
        }

        // Setup for the next scope
        self.state.tasks.store(1, Ordering::Relaxed);
        *finished = false;
    }

    fn spawn_worker(receiver: Arc<Mutex<Receiver<Task<'static>>>>, state: Arc<ThreadPoolState>) {
        std::thread::spawn(move || loop {
            let Ok(execute_task) = receiver.lock().unwrap().recv() else {
                break;
            };

            // Execute the task
            if panic::catch_unwind(panic::AssertUnwindSafe(execute_task)).is_err() {
                // Thread panicked while executing the task
                state.panicked.store(true, Ordering::Release);
            }

            // If there is still work to do
            if state.tasks.fetch_sub(1, Ordering::Release) > 1 {
                continue;
            }
            // Work is finished, we can signal back

            // This fence make sure other threads see the 'tasks' reaching '0' before the finished
            // signal is issued, meaning that all the tasks were actually finished. We use a fence
            // instead of `tasks.fetch_sub(1, Ordering::AcqRel)` to avoid paying the penalty on
            // every decrement.
            atomic::fence(Ordering::Acquire);
            *state.finished_mutex.lock().unwrap() = true;
            state.condvar.notify_one();
        });
    }
}

pub struct Scope<'scope, 'pool: 'scope> {
    threadpool: &'pool ThreadPool,
    scope: PhantomData<&'scope mut &'scope ()>,
}

impl<'scope, 'pool> Scope<'scope, 'pool> {
    pub fn enqueue_task<F>(&'scope self, f: F)
    where
        F: FnOnce() + Send + 'scope,
    {
        // SAFETY: We ensure that the thread finished executing 'f' before dropping the scope
        //         effectively ensuring 'f' lives at most `'scope` lifetime, even though it's
        //         `'static`.
        let f = unsafe { std::mem::transmute::<Task<'scope>, Task<'static>>(Box::new(f)) };

        self.threadpool.state.tasks.fetch_add(1, Ordering::Acquire);
        self.threadpool.sender.send(f).unwrap();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    #[should_panic]
    fn inner_panic_should_propagate() {
        let tp = ThreadPool::default();

        tp.with_scope(|scope| scope.enqueue_task(|| panic!()))
    }
}
