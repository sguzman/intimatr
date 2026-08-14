use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use thiserror::Error;
use tracing::{error, info, warn};

use crate::{
    config::{AppConfig, ConfigError},
    lifecycle::{Lifecycle, LifecycleError, LifecycleState},
    logging::{self, LoggingError, LoggingGuard},
};

#[cfg(windows)]
use crate::platform::windows::{self, WindowsError};

static LIFECYCLE: Lifecycle = Lifecycle::new();
static CONTEXT: Mutex<Option<RuntimeContext>> = Mutex::new(None);

pub struct RuntimeContext {
    pub config: AppConfig,
    pub host_executable: PathBuf,
    pub module_path: PathBuf,
    pub config_path: PathBuf,
    _logging_guard: LoggingGuard,
}

pub fn lifecycle_state() -> LifecycleState {
    LIFECYCLE.state()
}

pub fn shutdown_requested() -> bool {
    LIFECYCLE.shutdown_requested()
}

pub fn shutdown() -> Result<(), RuntimeError> {
    let should_cleanup = LIFECYCLE.begin_shutdown()?;
    if !should_cleanup {
        return Ok(());
    }

    info!(state = ?LIFECYCLE.state(), "Intimatr shutdown started");

    let context = {
        let mut guard = CONTEXT.lock().map_err(|_| RuntimeError::ContextPoisoned)?;
        guard.take()
    };

    if context.is_none() {
        warn!("Intimatr shutdown found no active runtime context");
    }

    LIFECYCLE.mark_stopped()?;
    info!(state = ?LIFECYCLE.state(), "Intimatr shutdown complete");

    drop(context);
    Ok(())
}

pub(crate) fn mark_attached() -> Result<(), RuntimeError> {
    LIFECYCLE.attach()?;
    Ok(())
}

pub(crate) fn mark_failed() {
    LIFECYCLE.mark_failed();
}

pub(crate) fn request_shutdown_from_loader() {
    LIFECYCLE.request_shutdown();
}

#[cfg(windows)]
pub(crate) fn bootstrap_windows(
    module: windows_sys::Win32::Foundation::HMODULE,
) -> Result<(), RuntimeError> {
    let module_path = windows::loaded_module_path(module)?;
    let host_executable = windows::current_process_executable()?;
    bootstrap(module_path, host_executable)
}

fn bootstrap(module_path: PathBuf, host_executable: PathBuf) -> Result<(), RuntimeError> {
    if LIFECYCLE.shutdown_requested() {
        let _ = LIFECYCLE.begin_shutdown();
        let _ = LIFECYCLE.mark_stopped();
        return Err(RuntimeError::ShutdownRequested);
    }

    {
        let guard = CONTEXT.lock().map_err(|_| RuntimeError::ContextPoisoned)?;
        if guard.is_some() {
            return Err(RuntimeError::AlreadyInitialized);
        }
    }

    LIFECYCLE.begin_bootstrap()?;

    match bootstrap_inner(module_path, host_executable) {
        Ok(context) => {
            if LIFECYCLE.shutdown_requested() {
                warn!("shutdown was requested while Intimatr was bootstrapping");
                LIFECYCLE.begin_shutdown()?;
                LIFECYCLE.mark_stopped()?;
                drop(context);
                return Err(RuntimeError::ShutdownRequested);
            }

            info!(
                module = %context.module_path.display(),
                host = %context.host_executable.display(),
                config = %context.config_path.display(),
                "Intimatr bootstrap completed"
            );

            {
                let mut guard = CONTEXT.lock().map_err(|_| RuntimeError::ContextPoisoned)?;
                *guard = Some(context);
            }

            LIFECYCLE.mark_running()?;
            info!(state = ?LIFECYCLE.state(), "Intimatr runtime is running");
            Ok(())
        }
        Err(error) => {
            LIFECYCLE.mark_failed();
            error!(state = ?LIFECYCLE.state(), error = %error, "Intimatr bootstrap failed");
            Err(error)
        }
    }
}

fn bootstrap_inner(
    module_path: PathBuf,
    host_executable: PathBuf,
) -> Result<RuntimeContext, RuntimeError> {
    let module_directory = module_directory(&module_path)?;
    let config_directory = module_directory.join("config");
    let config_path = AppConfig::config_path_for_executable(&config_directory, &host_executable)?;
    let mut config = AppConfig::load_for_executable(&config_directory, &host_executable)?;

    if config.logging.directory.is_relative() {
        config.logging.directory = module_directory.join(&config.logging.directory);
    }

    let logging_guard = logging::init(&config.logging)?;

    info!(
        module = %module_path.display(),
        host = %host_executable.display(),
        config = %config_path.display(),
        log_directory = %config.logging.directory.display(),
        rpc_enabled = config.rpc.enabled,
        debugger_enabled = config.debugger.enabled,
        ui_enabled = config.ui.enabled,
        allow_memory_read = config.policy.allow_memory_read,
        allow_memory_write = config.policy.allow_memory_write,
        allow_code_patch = config.policy.allow_code_patch,
        "Intimatr configuration and logging are online"
    );

    Ok(RuntimeContext {
        config,
        host_executable,
        module_path,
        config_path,
        _logging_guard: logging_guard,
    })
}

fn module_directory(module_path: &Path) -> Result<PathBuf, RuntimeError> {
    module_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| RuntimeError::MissingModuleDirectory(module_path.to_path_buf()))
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Logging(#[from] LoggingError),
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] WindowsError),
    #[error("Intimatr runtime context mutex was poisoned")]
    ContextPoisoned,
    #[error("Intimatr runtime is already initialized")]
    AlreadyInitialized,
    #[error("shutdown was requested before bootstrap completed")]
    ShutdownRequested,
    #[error("could not determine the parent directory of module {0}")]
    MissingModuleDirectory(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_directory_uses_parent_directory() {
        let path = Path::new(r"C:\Tools\Intimatr\intimatr.dll");
        let directory = module_directory(path).expect("module should have a parent directory");

        assert_eq!(directory, PathBuf::from(r"C:\Tools\Intimatr"));
    }
}
