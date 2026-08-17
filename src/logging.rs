use std::{
    fs, io,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::LoggingConfig;

pub struct LoggingGuard {
    file_guard: Option<WorkerGuard>,
}

impl LoggingGuard {
    pub fn flush(&mut self) {
        if let Some(guard) = self.file_guard.take() {
            drop(guard);
        }
    }
}

pub fn init(config: &LoggingConfig) -> Result<LoggingGuard, LoggingError> {
    fs::create_dir_all(&config.directory).map_err(|source| LoggingError::CreateDirectory {
        path: config.directory.clone(),
        source,
    })?;

    let run_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let run_file_name = build_run_file_name(&config.file_name, run_millis, process::id());

    let file_appender = tracing_appender::rolling::never(&config.directory, &run_file_name);
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_new(&config.filter)
        .map_err(|error| LoggingError::InvalidFilter(error.to_string()))?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_writer(file_writer);

    let console_layer = config.console.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|error| LoggingError::Initialize(error.to_string()))?;

    info!(
        directory = %config.directory.display(),
        configured_file = %config.file_name,
        run_file = %run_file_name,
        filter = %config.filter,
        console = config.console,
        "Intimatr logging initialized"
    );

    Ok(LoggingGuard {
        file_guard: Some(file_guard),
    })
}

fn build_run_file_name(configured: &str, run_millis: u128, process_id: u32) -> String {
    let configured = Path::new(configured);
    let stem = configured
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("intimatr");
    let suffix = format!("{run_millis}-pid{process_id}");

    match configured
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if !extension.is_empty() => format!("{stem}-{suffix}.{extension}"),
        _ => format!("{stem}-{suffix}"),
    }
}

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("failed to create log directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid tracing filter: {0}")]
    InvalidFilter(String),
    #[error("failed to initialize tracing subscriber: {0}")]
    Initialize(String),
}

#[cfg(test)]
mod tests {
    use super::build_run_file_name;

    #[test]
    fn run_log_name_preserves_configured_extension() {
        assert_eq!(
            build_run_file_name("intimatr.log", 1_234_567, 42),
            "intimatr-1234567-pid42.log"
        );
    }

    #[test]
    fn run_log_name_works_without_extension() {
        assert_eq!(build_run_file_name("session", 99, 7), "session-99-pid7");
    }
}
