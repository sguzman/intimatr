//! Windows focused-game hotkey *preview* worker.
//!
//! This thread never installs a global keyboard hook and never applies a
//! game-time speed modification. It owns no speed clock: actions go to the
//! same SpeedPreviewExecutor that serves RPC and UI commands. A window may
//! be sampled only when exactly one eligible HWND is verified on each tick.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use thiserror::Error;
use tracing::{debug, warn};

use crate::{
    speed_commands::{SpeedPreviewError, SpeedPreviewExecutor},
    speed_input::InputSnapshot,
    speed_input_windows::WindowsHotkeyInput,
    speed_window::FocusedWindowEpoch,
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum SpeedWorkerError {
    #[error(transparent)]
    Preview(#[from] SpeedPreviewError),
    #[error("cannot start focused hotkey preview thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error("focused hotkey preview thread panicked")]
    Join,
}

pub struct SpeedHotkeyWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SpeedHotkeyWorker {
    /// No bindings, no thread, no polling. The enabled preview profile is
    /// distinct from runtime timer-hook enrollment; it changes no game time.
    pub fn start(preview: Arc<SpeedPreviewExecutor>) -> Result<Option<Self>, SpeedWorkerError> {
        let keys = preview.watched_keys()?;
        if keys.is_empty() {
            return Ok(None);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("intimatr-focused-speed-preview".to_owned())
            .spawn(move || run_worker(preview, keys, thread_stop))?;
        Ok(Some(Self {
            stop,
            thread: Some(handle),
        }))
    }

    pub fn stop(&mut self) -> Result<(), SpeedWorkerError> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            handle.join().map_err(|_| SpeedWorkerError::Join)?;
        }
        Ok(())
    }
}

fn run_worker(preview: Arc<SpeedPreviewExecutor>, keys: Vec<u16>, stop: Arc<AtomicBool>) {
    debug!("focused game speed preview monitoring started");
    let mut epoch = FocusedWindowEpoch::default();
    while !stop.load(Ordering::Acquire) {
        // Refresh the window decision every tick. If a launcher, dialog or
        // tool window introduces ambiguity, no keyboard state is observed.
        let candidate = WindowsHotkeyInput::from_unique_game_window(&keys)
            .ok()
            .map(|reader| (reader.window_identity(), reader.poll()));
        // If the approved HWND changes without an intervening unfocused tick,
        // synthesize one focus loss before accepting the new window's keys.
        let snapshot = epoch.observe(candidate);
        if let Err(error) = preview.sample_input(snapshot) {
            warn!(error = %error, "focused speed preview stopped on shared-state error");
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    // Release any held speed before exiting, including on unexpected error.
    if let Err(error) = preview.sample_input(InputSnapshot::default()) {
        warn!(error = %error, "failed to reset focused speed preview after worker stop");
    }
}

impl Drop for SpeedHotkeyWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            warn!(error = %error, "focused speed preview thread did not stop cleanly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{Command, CommandError, CommandExecution, CommandExecutor},
        speed_config::SpeedSettings,
        speed_hotkeys::SpeedHotkeyConfig,
    };

    struct RejectAll;

    impl CommandExecutor for RejectAll {
        fn execute(&self, _: Command) -> Result<CommandExecution, CommandError> {
            Err(CommandError::NotImplemented("fixture"))
        }
    }

    #[test]
    fn disabled_and_unbound_preview_never_spawn_polling_worker() {
        let inner: Arc<dyn CommandExecutor> = Arc::new(RejectAll);
        let disabled = Arc::new(SpeedPreviewExecutor::disabled(Arc::clone(&inner)));
        assert!(SpeedHotkeyWorker::start(disabled).unwrap().is_none());
        let unbound = Arc::new(
            SpeedPreviewExecutor::with_profile(
                inner,
                &SpeedSettings {
                    enabled: true,
                    ..SpeedSettings::default()
                },
                &SpeedHotkeyConfig::default(),
                "test",
            )
            .unwrap(),
        );
        assert!(SpeedHotkeyWorker::start(unbound).unwrap().is_none());
    }
}
