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
use crate::{
    backpressure::{BoundedCommandExecutor, ExecutorPoolError},
    command::{CommandDispatcher, CommandExecutor, CommandLimits, PostAction},
    debugger_ui::{DebuggerUiError, DebuggerUiHandle},
    platform::windows::{self, WindowsError, memory::CurrentProcessMemory},
    rpc::{self, PostActionHandler, RpcServerError, RpcServerHandle},
    speed_commands::SpeedPreviewExecutor,
    speed_hotkey_worker::SpeedHotkeyWorker,
    speed_hud_telemetry::HudTelemetryWorker,
    speed_profile_load::wrap_explicit_preview_from_env,
    ui::{self, UiError, UiHandle},
};
#[cfg(windows)]
use std::sync::Arc;

static LIFECYCLE: Lifecycle = Lifecycle::new();
static CONTEXT: Mutex<Option<RuntimeContext>> = Mutex::new(None);

pub struct RuntimeContext {
    pub config: AppConfig,
    pub host_executable: PathBuf,
    pub module_path: PathBuf,
    pub config_path: PathBuf,
    #[cfg(windows)]
    command_executor: Arc<dyn CommandExecutor>,
    #[cfg(windows)]
    speed_preview_executor: Arc<SpeedPreviewExecutor>,
    #[cfg(windows)]
    _ui: Option<UiHandle>,
    #[cfg(windows)]
    _debugger_ui: Option<DebuggerUiHandle>,
    #[cfg(windows)]
    _rpc_server: Option<RpcServerHandle>,
    #[cfg(windows)]
    _speed_hotkey_worker: Option<SpeedHotkeyWorker>,
    #[cfg(windows)]
    _hud_telemetry: Option<HudTelemetryWorker>,
    _logging_guard: LoggingGuard,
}

pub fn lifecycle_state() -> LifecycleState {
    LIFECYCLE.state()
}

pub fn shutdown_requested() -> bool {
    LIFECYCLE.shutdown_requested()
}

pub fn shutdown() -> Result<(), RuntimeError> {
    // Close the activation gate before restoring imports. Otherwise a
    // concurrent enable can acquire the hook mutex after restoration but
    // before begin_shutdown marks the lifecycle as Stopping.
    LIFECYCLE.request_shutdown();
    // Never report Stopped while this DLL still owns a live executable IAT
    // callback. Do this outside the loader lock and before lifecycle teardown
    // so a failed restore can be retried without falsely completing shutdown.
    #[cfg(windows)]
    if !crate::experimental_timer_probe::disable_probe() {
        return Err(RuntimeError::TimerHooksStillActive);
    }
    let should_cleanup = LIFECYCLE.begin_shutdown()?;
    if !should_cleanup {
        return Ok(());
    }

    info!(state = ?LIFECYCLE.state(), "Intimatr shutdown started");

    let mut context = {
        let mut guard = CONTEXT.lock().map_err(|_| RuntimeError::ContextPoisoned)?;
        guard.take()
    };

    if let Some(context) = context.as_mut() {
        #[cfg(windows)]
        {
            if let Some(mut rpc_server) = context._rpc_server.take()
                && let Err(error) = rpc_server.stop()
            {
                warn!(error = %error, "failed to stop RPC server during shutdown");
            }
            if let Some(mut debugger_ui) = context._debugger_ui.take()
                && let Err(error) = debugger_ui.stop()
            {
                warn!(error = %error, "failed to stop debugger UI during shutdown");
            }
            if let Some(mut ui) = context._ui.take()
                && let Err(error) = ui.stop()
            {
                warn!(error = %error, "failed to stop main UI during shutdown");
            }
            if let Some(mut worker) = context._speed_hotkey_worker.take()
                && let Err(error) = worker.stop()
            {
                warn!(error = %error, "failed to stop focused speed preview on shutdown");
            }
            if let Some(mut worker) = context._hud_telemetry.take()
                && let Err(error) = worker.stop()
            {
                warn!(error = %error, "failed to stop HUD telemetry during shutdown");
            }
            context.command_executor.shutdown();
        }
    } else {
        warn!("Intimatr shutdown found no active runtime context");
    }

    LIFECYCLE.mark_stopped()?;
    info!(state = ?LIFECYCLE.state(), "Intimatr shutdown complete");

    if let Some(context) = context.as_mut() {
        context._logging_guard.flush();
    }
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
            // A proxy load does not grant permission to arm game timing.
            #[cfg(windows)]
            maybe_arm_explicit_startup_timers();
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
        rpc_transport = ?config.rpc.transport,
        command_workers = config.runtime.command_workers,
        command_queue_capacity = config.runtime.command_queue_capacity,
        scanner_worker_threads = config.scanner.worker_threads,
        debugger_enabled = config.debugger.enabled,
        debugger_ui_enabled = config.debugger.ui_enabled,
        debugger_ui_toggle_key = %config.debugger.ui_toggle_key,
        ui_enabled = config.ui.enabled,
        ui_toggle_key = %config.ui.toggle_key,
        allow_memory_read = config.policy.allow_memory_read,
        allow_memory_write = config.policy.allow_memory_write,
        allow_code_patch = config.policy.allow_code_patch,
        allow_debugger = config.policy.allow_debugger,
        "Intimatr configuration and logging are online"
    );

    #[cfg(windows)]
    let SpeedExecutorBundle {
        executor: command_executor,
        hotkey_worker,
        preview: speed_preview_executor,
    } = create_command_executor(&config, &module_directory)?;
    #[cfg(windows)]
    let rpc_server = start_rpc_if_enabled(&config, Arc::clone(&command_executor))?;
    #[cfg(windows)]
    let ui = start_ui_if_enabled(&config, &module_directory, Arc::clone(&command_executor))?;
    #[cfg(windows)]
    let debugger_ui =
        start_debugger_ui_if_enabled(&config, &module_directory, Arc::clone(&command_executor))?;
    // A host overlay reads only the observed, already-installed virtual rate.
    // This publisher never enables timer hooks or touches render APIs.
    #[cfg(windows)]
    let hud_telemetry = match HudTelemetryWorker::start(&module_directory) {
        Ok(worker) => worker,
        Err(error) => {
            warn!(error = %error, "opt-in Intimatr HUD telemetry unavailable");
            None
        }
    };

    Ok(RuntimeContext {
        config,
        host_executable,
        module_path,
        config_path,
        #[cfg(windows)]
        command_executor,
        #[cfg(windows)]
        speed_preview_executor,
        #[cfg(windows)]
        _ui: ui,
        #[cfg(windows)]
        _debugger_ui: debugger_ui,
        #[cfg(windows)]
        _rpc_server: rpc_server,
        #[cfg(windows)]
        _speed_hotkey_worker: hotkey_worker,
        #[cfg(windows)]
        _hud_telemetry: hud_telemetry,
        _logging_guard: logging_guard,
    })
}

#[cfg(windows)]
struct SpeedExecutorBundle {
    executor: Arc<dyn CommandExecutor>,
    hotkey_worker: Option<SpeedHotkeyWorker>,
    preview: Arc<SpeedPreviewExecutor>,
}

#[cfg(windows)]
fn create_command_executor(
    config: &AppConfig,
    module_directory: &Path,
) -> Result<SpeedExecutorBundle, RuntimeError> {
    let analysis_directory = module_directory
        .join("analysis")
        .join(&config.target.executable);
    let dispatcher: Arc<dyn CommandExecutor> = Arc::new(
        CommandDispatcher::new_with_debugger(
            CurrentProcessMemory::new(),
            config.scanner.clone(),
            config.debugger.clone(),
            config.policy.clone(),
            CommandLimits {
                max_memory_transfer_bytes: config.rpc.max_memory_transfer_bytes,
                max_scan_results_per_page: config.rpc.max_scan_results_per_page,
            },
        )
        .with_analysis_directory(analysis_directory),
    );
    let bounded = BoundedCommandExecutor::new(
        dispatcher,
        config.runtime.command_workers,
        config.runtime.command_queue_capacity,
    )?;
    let executor: Arc<dyn CommandExecutor> = bounded;
    // The shared profile path must be explicitly supplied by the loader.
    // Invalid/missing profiles disable speed only, retaining all legacy tools.
    let preview = wrap_explicit_preview_from_env(executor);
    let worker = match SpeedHotkeyWorker::start(Arc::clone(&preview)) {
        Ok(worker) => worker,
        Err(error) => {
            warn!(error = %error, "cannot start focused speed preview; scanner and RPC still available");
            None
        }
    };
    let executor: Arc<dyn CommandExecutor> = Arc::clone(&preview) as Arc<dyn CommandExecutor>;
    Ok(SpeedExecutorBundle {
        executor,
        hotkey_worker: worker,
        preview,
    })
}

#[cfg(windows)]
fn start_rpc_if_enabled(
    config: &AppConfig,
    executor: Arc<dyn CommandExecutor>,
) -> Result<Option<RpcServerHandle>, RuntimeError> {
    if !config.rpc.enabled {
        info!("RPC server is disabled by configuration");
        return Ok(None);
    }

    let post_action_handler: PostActionHandler = Arc::new(handle_rpc_post_action);
    let server = rpc::start_server(config.rpc.clone(), executor, post_action_handler)?;
    info!(endpoint = ?server.endpoint(), "Intimatr RPC server is online");
    Ok(Some(server))
}

#[cfg(windows)]
fn start_ui_if_enabled(
    config: &AppConfig,
    module_directory: &Path,
    executor: Arc<dyn CommandExecutor>,
) -> Result<Option<UiHandle>, RuntimeError> {
    if !config.ui.enabled {
        info!("in-process UI is disabled by configuration");
        return Ok(None);
    }

    let persistence_path = module_directory.join("ui").join(&config.target.executable);
    let handle = ui::UiHandle::start(
        config.ui.clone(),
        config.target.executable.clone(),
        persistence_path.clone(),
        executor,
    )?;
    info!(
        toggle_key = %config.ui.toggle_key,
        persistence_path = %persistence_path.display(),
        "Intimatr in-process UI thread started"
    );
    Ok(Some(handle))
}

#[cfg(windows)]
fn start_debugger_ui_if_enabled(
    config: &AppConfig,
    module_directory: &Path,
    executor: Arc<dyn CommandExecutor>,
) -> Result<Option<DebuggerUiHandle>, RuntimeError> {
    if !config.debugger.enabled || !config.debugger.ui_enabled {
        info!("debugger UI is disabled by configuration");
        return Ok(None);
    }
    if !config.policy.allow_debugger {
        info!("debugger UI is disabled because policy.allow_debugger is false");
        return Ok(None);
    }

    let persistence_path = module_directory
        .join("ui")
        .join(&config.target.executable)
        .join("debugger");
    let handle = DebuggerUiHandle::start(
        config.debugger.clone(),
        config.target.executable.clone(),
        persistence_path.clone(),
        executor,
    )?;
    info!(
        toggle_key = %config.debugger.ui_toggle_key,
        persistence_path = %persistence_path.display(),
        "Intimatr debugger UI thread started"
    );
    Ok(Some(handle))
}

/// Observe how the target DLL's own bootstrap classified the launch speed
/// profile. This is read-only: 0=absent, 1=accepted, 2=present but rejected
/// or disabled, 3=runtime unavailable. No path or environment is disclosed.
#[cfg(windows)]
pub(crate) fn remote_explicit_speed_profile_state() -> u32 {
    if lifecycle_state() != LifecycleState::Running {
        return 3;
    }
    CONTEXT
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().map(|context| {
                context
                    .speed_preview_executor
                    .explicit_launch_profile_state()
            })
        })
        .unwrap_or(3)
}

/// A single SHA-256 word from the profile accepted at target bootstrap.
/// A zero return without accepted profile state is NOT digest evidence.
/// Selector is an integer 0..7, never a dereferenceable pointer.
#[cfg(windows)]
pub(crate) fn remote_speed_profile_sha256_word(selector: usize) -> u32 {
    if selector >= 8 || lifecycle_state() != LifecycleState::Running {
        return 0;
    }
    CONTEXT
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().and_then(|context| {
                context
                    .speed_preview_executor
                    .explicit_profile_sha256_word(selector)
            })
        })
        .unwrap_or(0)
}

/// Require a Running runtime and atomically installed IAT before enabling
/// the one shared speed executor. Roll back imports if command setup fails.
#[cfg(windows)]
pub(crate) fn arm_selected_game_timers() -> bool {
    if lifecycle_state() != LifecycleState::Running || shutdown_requested() {
        return false;
    }
    let preview = CONTEXT.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|context| Arc::clone(&context.speed_preview_executor))
    });
    let Some(preview) = preview else {
        return false;
    };
    let Ok(settings) = preview.selected_game_timer_settings() else {
        return false;
    };
    if !crate::experimental_timer_probe::enable_selected_game_timers(&settings) {
        return false;
    }
    if preview.arm_selected_game_session().is_ok() {
        return true;
    }
    let _ = crate::experimental_timer_probe::disable_probe();
    false
}

/// Convert an already-validated speed to the remote thousandth-rate ABI.
/// Truncation is incorrect for valid decimal steps such as 1.001x: a binary
/// floating-point product may be 1000.99999 even though the selected rate is
/// exactly 1001 thousandths. Keep preset and step confirmations consistent.
#[cfg(any(test, windows))]
fn rounded_speed_rate_milli(multiplier: f64) -> u32 {
    (multiplier * 1000.0).round() as u32
}

/// Selected-game speed uses the same speed CommandExecutor as RPC/UI. A
/// report is successful only when the installed timer session agrees.
#[cfg(windows)]
pub(crate) fn selected_game_select_rate(multiplier: f64) -> bool {
    use crate::{
        command::{Command, CommandResult},
        speed_commands::SpeedRequest,
    };
    if shutdown_requested()
        || lifecycle_state() != LifecycleState::Running
        || crate::experimental_timer_probe::remote_hook_state() != 1
    {
        return false;
    }
    let executor = CONTEXT.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|context| Arc::clone(&context.command_executor))
    });
    let Some(executor) = executor else {
        return false;
    };
    let Ok(result) = executor.execute(Command::Speed {
        request: SpeedRequest::SelectPreset { multiplier },
    }) else {
        return false;
    };
    let CommandResult::Speed { status } = result.result else {
        return false;
    };
    status.configured
        && status.preview_multiplier == multiplier
        && crate::experimental_timer_probe::remote_rate_milli()
            == rounded_speed_rate_milli(multiplier)
}

/// Apply one configured shared speed-state action to the exact selected
/// already-armed game timer session. No action activates hooks or proves that
/// actual gameplay simulation timing has changed. Returns the independently
/// observed installed virtual-clock rate, in thousandths.
#[cfg(windows)]
pub(crate) fn selected_game_step_action(selector: u32) -> u32 {
    use crate::{
        command::{Command, CommandResult},
        speed_commands::SpeedRequest,
    };
    let request = match selector {
        1 => SpeedRequest::Increase,
        2 => SpeedRequest::Decrease,
        3 => SpeedRequest::Toggle,
        4 => SpeedRequest::Reset,
        _ => return 0,
    };
    if shutdown_requested()
        || lifecycle_state() != LifecycleState::Running
        || crate::experimental_timer_probe::remote_hook_state() != 1
    {
        return 0;
    }
    let executor = CONTEXT.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|context| Arc::clone(&context.command_executor))
    });
    let Some(executor) = executor else {
        return 0;
    };
    let Ok(result) = executor.execute(Command::Speed { request }) else {
        return 0;
    };
    let CommandResult::Speed { status } = result.result else {
        return 0;
    };
    let observed = crate::experimental_timer_probe::remote_rate_milli();
    let expected = rounded_speed_rate_milli(status.preview_multiplier);
    if !status.configured || observed != expected || !(100..=10_000).contains(&observed) {
        return 0;
    }
    observed
}

/// Fixture-only bridge to the *same* command executor used by RPC/UI.
/// It does not install a hook or assert that game time is accelerated.
/// The experimental hook module additionally requires its allowlisted
/// disposable EXE and already-active four-API IAT hook before calling this.
#[cfg(windows)]
pub(crate) fn fixture_preview_select_rate(multiplier: f64) -> Option<f64> {
    use crate::{
        command::{Command, CommandResult},
        speed_commands::SpeedRequest,
    };
    let executor = {
        let guard = CONTEXT.lock().ok()?;
        Arc::clone(&guard.as_ref()?.command_executor)
    };
    let result = executor
        .execute(Command::Speed {
            request: SpeedRequest::SelectPreset { multiplier },
        })
        .ok()?;
    let CommandResult::Speed { status } = result.result else {
        return None;
    };
    status.configured.then_some(status.preview_multiplier)
}

#[cfg(windows)]
fn handle_rpc_post_action(action: PostAction) {
    match action {
        PostAction::Shutdown => {
            let spawn_result = std::thread::Builder::new()
                .name("intimatr-rpc-shutdown".to_owned())
                .spawn(|| {
                    if let Err(error) = shutdown() {
                        error!(error = %error, "RPC-triggered shutdown failed");
                    }
                });
            if let Err(error) = spawn_result {
                error!(error = %error, "failed to spawn RPC shutdown worker");
            }
        }
    }
}

/// Load-time hooks require an exact opt-in and an accepted launch profile.
#[cfg(any(test, windows))]
fn approved_startup_timer_activation(flag: Option<&str>, profile_state: u32) -> bool {
    flag == Some("1") && profile_state == 1
}

#[cfg(windows)]
fn maybe_arm_explicit_startup_timers() {
    const FLAG: &str = "INTIMATR_EXPERIMENTAL_STARTUP_TIMERS";
    let consent = std::env::var(FLAG).ok();
    if consent.as_deref() != Some("1") {
        return;
    }
    let profile_state = remote_explicit_speed_profile_state();
    if !approved_startup_timer_activation(consent.as_deref(), profile_state) {
        warn!(
            profile_state,
            "startup timer hooks refused: explicit launch profile not accepted"
        );
        return;
    }
    if arm_selected_game_timers() {
        info!("startup timer hooks installed at safe initial 1x");
    } else {
        warn!("startup timer hooking failed; game timing remains unmodified");
    }
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
    ExecutorPool(#[from] ExecutorPoolError),
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] WindowsError),
    #[cfg(windows)]
    #[error(transparent)]
    Rpc(#[from] RpcServerError),
    #[cfg(windows)]
    #[error(transparent)]
    Ui(#[from] UiError),
    #[cfg(windows)]
    #[error(transparent)]
    DebuggerUi(#[from] DebuggerUiError),
    #[cfg(windows)]
    #[error("experimental timer hooks could not be fully restored; refuse to stop or unload")]
    TimerHooksStillActive,
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
    fn remote_thousandth_rate_confirmation_roundtrips_every_supported_step() {
        // Each rate may be represented inexactly as f64, but must compare
        // against the exact ABI integer (0.100x through 10.000x).
        for milli in 100_u32..=10_000 {
            let rate = f64::from(milli) / 1000.0;
            assert_eq!(rounded_speed_rate_milli(rate), milli, "{rate}x");
        }
    }

    #[test]
    fn startup_timer_hooks_require_exact_flag_and_accepted_profile() {
        for flag in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some(" 1"),
            Some("1 "),
        ] {
            assert!(!approved_startup_timer_activation(flag, 1));
        }
        for state in [0, 2, 3] {
            assert!(!approved_startup_timer_activation(Some("1"), state));
        }
        assert!(approved_startup_timer_activation(Some("1"), 1));
    }

    #[test]
    fn module_directory_uses_parent_directory() {
        let path = Path::new(r"C:\Tools\Intimatr\intimatr.dll");
        let directory = module_directory(path).expect("module should have a parent directory");

        assert_eq!(directory, PathBuf::from(r"C:\Tools\Intimatr"));
    }
}
