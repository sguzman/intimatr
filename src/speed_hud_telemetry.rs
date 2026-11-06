//! Opt-in, read-only export of the *installed* virtual-clock speed for the
//! Linux MangoHud adapter. No graphics API hooks, network traffic or input.
//!
//! Written to the selected game directory, which the native Bottles launcher
//! can independently address from the Linux host. An unavailable hook is "--",
//! never a fabricated 1x or an assumed applied gameplay rate.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use thiserror::Error;

const NAME: &str = "intimatr-hud-rate.txt";
const INTERVAL: Duration = Duration::from_millis(250);
const OPT_IN: &str = "INTIMATR_HUD_TELEMETRY";

#[derive(Debug, Error)]
pub enum HudTelemetryError {
    #[error("HUD rate output path is a symbolic link or not a regular file")]
    UnsafeOutput,
    #[error("HUD rate output I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HUD rate worker thread panicked")]
    Join,
}

/// Show only the currently installed hook's own rate. None/zero means that
/// acceleration is unavailable, *not* that the game is running at 1.00x.
pub fn format_observed_rate(rate_milli: u32) -> String {
    if !(100..=10_000).contains(&rate_milli) {
        return "--".to_owned();
    }
    format!("{:.2}x", f64::from(rate_milli) / 1000.0)
}

fn refuse_nonregular_existing_file(path: &Path) -> Result<(), HudTelemetryError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            Err(HudTelemetryError::UnsafeOutput)
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn write_rate(path: &Path, text: &str) -> Result<(), HudTelemetryError> {
    refuse_nonregular_existing_file(path)?;
    // The fixed filename lives in the already-approved game module directory.
    // Keep the payload short; MangoHud reads it as plain text from the host.
    fs::write(path, text)?;
    Ok(())
}

pub struct HudTelemetryWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HudTelemetryWorker {
    /// Never enabled by default; the launcher must opt in with an exact 1.
    /// Failure must never prevent the game from booting.
    pub fn start(module_directory: &Path) -> Result<Option<Self>, HudTelemetryError> {
        // Reuse the established explicit startup opt-in for the common
        // Bottles speed HUD. A dedicated telemetry opt-in also works without
        // arming hooks and correctly displays "--".
        if std::env::var(OPT_IN).ok().as_deref() != Some("1")
            && std::env::var("INTIMATR_EXPERIMENTAL_STARTUP_TIMERS")
                .ok()
                .as_deref()
                != Some("1")
        {
            return Ok(None);
        }
        let path = module_directory.join(NAME);
        refuse_nonregular_existing_file(&path)?;
        // Initialize before spawning so that missing hooks cannot leave the
        // previous game's stale rate visible.
        write_rate(&path, "--")?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("intimatr-hud-telemetry".into())
            .spawn(move || run(path, thread_stop))?;
        Ok(Some(Self {
            stop,
            thread: Some(handle),
        }))
    }

    pub fn stop(&mut self) -> Result<(), HudTelemetryError> {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            worker.join().map_err(|_| HudTelemetryError::Join)?;
        }
        Ok(())
    }
}

fn run(path: PathBuf, stop: Arc<AtomicBool>) {
    let mut previous = String::new();
    while !stop.load(Ordering::Acquire) {
        let observed = if crate::experimental_timer_probe::remote_hook_state() == 1 {
            crate::experimental_timer_probe::remote_rate_milli()
        } else {
            0
        };
        let text = format_observed_rate(observed);
        if text != previous {
            if write_rate(&path, &text).is_err() {
                // Never crash the game or write through an unexpected link.
                break;
            }
            previous = text;
        }
        thread::sleep(INTERVAL);
    }
    let _ = write_rate(&path, "--");
}

impl Drop for HudTelemetryWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::format_observed_rate;

    #[test]
    fn distinguishes_unavailable_from_real_installed_speed() {
        assert_eq!(format_observed_rate(0), "--");
        assert_eq!(format_observed_rate(99), "--");
        assert_eq!(format_observed_rate(1000), "1.00x");
        assert_eq!(format_observed_rate(1500), "1.50x");
        assert_eq!(format_observed_rate(2000), "2.00x");
        assert_eq!(format_observed_rate(3000), "3.00x");
        assert_eq!(format_observed_rate(10_001), "--");
    }
}
