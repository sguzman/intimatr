use std::{fs, io, path::PathBuf};

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

    let file_appender = tracing_appender::rolling::never(&config.directory, &config.file_name);
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
        file = %config.file_name,
        filter = %config.filter,
        console = config.console,
        "Intimatr logging initialized"
    );

    Ok(LoggingGuard {
        file_guard: Some(file_guard),
    })
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
