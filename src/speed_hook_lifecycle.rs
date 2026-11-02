//! Bounded in-flight callback bookkeeping for experimental timing hooks.
//!
//! A restored IAT slot does not imply that previously entered callbacks have
//! finished. This helper makes that additional shutdown precondition explicit.
//! It cannot detect a thread that fetched an old IAT pointer but has not yet
//! entered the callback, so it is not sufficient for arbitrary-game unload.
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
pub struct CallbackTracker {
    active: AtomicUsize,
}

#[must_use]
pub struct CallbackGuard<'a> {
    tracker: &'a CallbackTracker,
}

impl CallbackTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
        }
    }

    pub fn enter(&self) -> CallbackGuard<'_> {
        self.active.fetch_add(1, Ordering::AcqRel);
        CallbackGuard { tracker: self }
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Called only after the relevant import pointers have been restored.
    /// Timeouts are a failed shutdown precondition, never implicit success.
    #[must_use]
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.active_count() == 0 {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            thread::yield_now();
        }
    }
}

impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        self.tracker.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn active_callback_blocks_shutdown_until_guard_is_released() {
        let tracker = CallbackTracker::default();
        let guard = tracker.enter();
        assert_eq!(tracker.active_count(), 1);
        assert!(!tracker.wait_until_idle(Duration::ZERO));
        drop(guard);
        assert!(tracker.wait_until_idle(Duration::ZERO));
    }

    #[test]
    fn multiple_threads_are_counted_until_the_last_callback_exits() {
        let tracker = Arc::new(CallbackTracker::default());
        let arrived = Arc::new(Barrier::new(3));
        let finish = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let tracker = Arc::clone(&tracker);
            let arrived = Arc::clone(&arrived);
            let finish = Arc::clone(&finish);
            workers.push(thread::spawn(move || {
                let _guard = tracker.enter();
                arrived.wait();
                finish.wait();
            }));
        }
        arrived.wait();
        assert_eq!(tracker.active_count(), 2);
        assert!(!tracker.wait_until_idle(Duration::ZERO));
        finish.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(tracker.active_count(), 0);
        assert!(tracker.wait_until_idle(Duration::from_millis(1)));
    }
}
