use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::command::{Command, CommandError, CommandExecution, CommandExecutor};

enum WorkItem {
    Execute {
        command: Command,
        response: SyncSender<Result<CommandExecution, CommandError>>,
    },
    Stop,
}

/// A bounded, fixed-size worker pool in front of the shared command executor.
///
/// Every frontend uses the same instance at runtime. The synchronous queue is
/// deliberately bounded: producers block when the queue is full instead of
/// creating unbounded command tasks or memory pressure inside the target.
pub struct BoundedCommandExecutor {
    inner: Arc<dyn CommandExecutor>,
    sender: SyncSender<WorkItem>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    submit_lock: Mutex<()>,
    stopping: Arc<AtomicBool>,
    queue_capacity: usize,
}

impl BoundedCommandExecutor {
    pub fn new(
        inner: Arc<dyn CommandExecutor>,
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Arc<Self>, ExecutorPoolError> {
        if worker_count == 0 {
            return Err(ExecutorPoolError::InvalidWorkerCount);
        }
        if queue_capacity == 0 {
            return Err(ExecutorPoolError::InvalidQueueCapacity);
        }

        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let stopping = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let worker_receiver = Arc::clone(&receiver);
            let worker_inner = Arc::clone(&inner);
            let worker_stopping = Arc::clone(&stopping);
            let handle = thread::Builder::new()
                .name(format!("intimatr-command-{index}"))
                .spawn(move || {
                    run_worker(index, worker_inner, worker_receiver, worker_stopping);
                })
                .map_err(ExecutorPoolError::ThreadSpawn)?;
            workers.push(handle);
        }

        info!(
            worker_count,
            queue_capacity, "bounded shared command executor started"
        );
        Ok(Arc::new(Self {
            inner,
            sender,
            workers: Mutex::new(workers),
            submit_lock: Mutex::new(()),
            stopping,
            queue_capacity,
        }))
    }

    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    pub fn worker_count(&self) -> usize {
        self.workers
            .lock()
            .map(|workers| workers.len())
            .unwrap_or_default()
    }

    fn stop_workers(&self) {
        let worker_count = self.worker_count();
        for _ in 0..worker_count {
            if self.sender.send(WorkItem::Stop).is_err() {
                break;
            }
        }

        let workers = match self.workers.lock() {
            Ok(mut workers) => std::mem::take(&mut *workers),
            Err(poisoned) => {
                warn!("command worker handle mutex was poisoned during shutdown");
                let mut workers = poisoned.into_inner();
                std::mem::take(&mut *workers)
            }
        };
        for worker in workers {
            if worker.join().is_err() {
                error!("bounded command worker panicked during shutdown");
            }
        }
    }
}

impl CommandExecutor for BoundedCommandExecutor {
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        let submit_guard = self
            .submit_lock
            .lock()
            .map_err(|_| CommandError::StatePoisoned)?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(CommandError::StatePoisoned);
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        let command_name = command.name();
        self.sender
            .send(WorkItem::Execute {
                command,
                response: response_tx,
            })
            .map_err(|_| CommandError::StatePoisoned)?;
        drop(submit_guard);

        debug!(
            command = command_name,
            "submitted command to bounded executor"
        );
        response_rx
            .recv()
            .map_err(|_| CommandError::StatePoisoned)?
    }

    fn shutdown(&self) {
        let submit_guard = self
            .submit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        drop(submit_guard);

        info!("bounded command executor shutdown started");
        self.inner.shutdown();
        self.stop_workers();
        info!("bounded command executor shutdown complete");
    }
}

impl Drop for BoundedCommandExecutor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_worker(
    index: usize,
    inner: Arc<dyn CommandExecutor>,
    receiver: Arc<Mutex<Receiver<WorkItem>>>,
    stopping: Arc<AtomicBool>,
) {
    loop {
        let item = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(poisoned) => {
                warn!(worker = index, "command queue receiver mutex was poisoned");
                poisoned.into_inner().recv()
            }
        };
        match item {
            Ok(WorkItem::Execute { command, response }) => {
                let result = if stopping.load(Ordering::Acquire) {
                    Err(CommandError::StatePoisoned)
                } else {
                    inner.execute(command)
                };
                let _ = response.send(result);
            }
            Ok(WorkItem::Stop) | Err(_) => break,
        }
    }
    debug!(worker = index, "bounded command worker exited");
}

#[derive(Debug, Error)]
pub enum ExecutorPoolError {
    #[error("runtime.command_workers must be greater than zero")]
    InvalidWorkerCount,
    #[error("runtime.command_queue_capacity must be greater than zero")]
    InvalidQueueCapacity,
    #[error("failed to spawn bounded command worker: {0}")]
    ThreadSpawn(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::command::{CommandExecution, CommandResult};

    struct CountingExecutor {
        active: AtomicUsize,
        maximum_active: AtomicUsize,
        shutdowns: AtomicUsize,
    }

    impl CountingExecutor {
        fn new() -> Self {
            Self {
                active: AtomicUsize::new(0),
                maximum_active: AtomicUsize::new(0),
                shutdowns: AtomicUsize::new(0),
            }
        }
    }

    impl CommandExecutor for CountingExecutor {
        fn execute(&self, _command: Command) -> Result<CommandExecution, CommandError> {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.maximum_active.fetch_max(active, Ordering::AcqRel);
            thread::sleep(Duration::from_millis(15));
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(CommandExecution {
                result: CommandResult::Pong,
                post_action: None,
            })
        }

        fn shutdown(&self) {
            self.shutdowns.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[test]
    fn bounds_command_parallelism_and_drains_results() {
        let inner = Arc::new(CountingExecutor::new());
        let inner_dyn: Arc<dyn CommandExecutor> = inner.clone();
        let pool = BoundedCommandExecutor::new(inner_dyn, 2, 2).unwrap();
        let start = Arc::new(Barrier::new(9));
        let mut callers = Vec::new();

        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let start = Arc::clone(&start);
            callers.push(thread::spawn(move || {
                start.wait();
                let execution = pool.execute(Command::Ping).unwrap();
                assert!(matches!(execution.result, CommandResult::Pong));
            }));
        }
        start.wait();
        for caller in callers {
            caller.join().unwrap();
        }

        assert!(inner.maximum_active.load(Ordering::Acquire) <= 2);
        pool.shutdown();
    }

    #[test]
    fn shutdown_is_idempotent() {
        let inner = Arc::new(CountingExecutor::new());
        let inner_dyn: Arc<dyn CommandExecutor> = inner.clone();
        let pool = BoundedCommandExecutor::new(inner_dyn, 1, 1).unwrap();
        pool.shutdown();
        pool.shutdown();
        assert_eq!(inner.shutdowns.load(Ordering::Acquire), 1);
    }

    #[test]
    fn rejects_zero_sized_pool_configuration() {
        let inner: Arc<dyn CommandExecutor> = Arc::new(CountingExecutor::new());
        assert!(matches!(
            BoundedCommandExecutor::new(Arc::clone(&inner), 0, 1),
            Err(ExecutorPoolError::InvalidWorkerCount)
        ));
        assert!(matches!(
            BoundedCommandExecutor::new(inner, 1, 0),
            Err(ExecutorPoolError::InvalidQueueCapacity)
        ));
    }
}
