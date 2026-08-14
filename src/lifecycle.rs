use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LifecycleState {
    Detached = 0,
    Attached = 1,
    Bootstrapping = 2,
    Running = 3,
    Stopping = 4,
    Stopped = 5,
    Failed = 6,
}

impl LifecycleState {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Detached,
            1 => Self::Attached,
            2 => Self::Bootstrapping,
            3 => Self::Running,
            4 => Self::Stopping,
            5 => Self::Stopped,
            6 => Self::Failed,
            _ => Self::Failed,
        }
    }
}

pub struct Lifecycle {
    state: AtomicU8,
    shutdown_requested: AtomicBool,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(LifecycleState::Detached as u8),
            shutdown_requested: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> LifecycleState {
        LifecycleState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::Acquire)
    }

    pub fn attach(&self) -> Result<(), LifecycleError> {
        self.shutdown_requested.store(false, Ordering::Release);
        self.transition(LifecycleState::Detached, LifecycleState::Attached)
    }

    pub fn begin_bootstrap(&self) -> Result<(), LifecycleError> {
        self.transition(LifecycleState::Attached, LifecycleState::Bootstrapping)
    }

    pub fn mark_running(&self) -> Result<(), LifecycleError> {
        self.transition(LifecycleState::Bootstrapping, LifecycleState::Running)
    }

    pub fn mark_failed(&self) {
        loop {
            let current = self.state();
            if matches!(current, LifecycleState::Stopping | LifecycleState::Stopped) {
                return;
            }

            if self
                .state
                .compare_exchange(
                    current.as_u8(),
                    LifecycleState::Failed.as_u8(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn request_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Release);
    }

    pub fn begin_shutdown(&self) -> Result<bool, LifecycleError> {
        self.request_shutdown();

        loop {
            let current = self.state();
            match current {
                LifecycleState::Detached => {
                    if self
                        .state
                        .compare_exchange(
                            current.as_u8(),
                            LifecycleState::Stopped.as_u8(),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return Ok(false);
                    }
                }
                LifecycleState::Stopping | LifecycleState::Stopped => return Ok(false),
                LifecycleState::Attached
                | LifecycleState::Bootstrapping
                | LifecycleState::Running
                | LifecycleState::Failed => {
                    if self
                        .state
                        .compare_exchange(
                            current.as_u8(),
                            LifecycleState::Stopping.as_u8(),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return Ok(true);
                    }
                }
            }
        }
    }

    pub fn mark_stopped(&self) -> Result<(), LifecycleError> {
        match self.state() {
            LifecycleState::Stopped => Ok(()),
            LifecycleState::Stopping => {
                self.transition(LifecycleState::Stopping, LifecycleState::Stopped)
            }
            current => Err(LifecycleError::InvalidTransition {
                from: current,
                to: LifecycleState::Stopped,
            }),
        }
    }

    fn transition(
        &self,
        expected: LifecycleState,
        next: LifecycleState,
    ) -> Result<(), LifecycleError> {
        self.state
            .compare_exchange(
                expected.as_u8(),
                next.as_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|actual| LifecycleError::InvalidTransition {
                from: LifecycleState::from_u8(actual),
                to: next,
            })
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LifecycleError {
    #[error("invalid lifecycle transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: LifecycleState,
        to: LifecycleState,
    },
}
