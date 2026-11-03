//! Frontend-neutral speed command boundary. Ordinary processes stay preview-only;
//! only explicitly armed offline-game IAT sessions receive applied actions.
//!
//! An opt-in wrapper around the existing CommandExecutor. It forwards every
//! legacy command unchanged and serializes preview speed/UI/RPC/key actions
//! through exactly one SpeedCoordinator. Nothing here installs timer hooks.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    command::{Command, CommandError, CommandExecution, CommandExecutor, CommandResult},
    speed_config::SpeedSettings,
    speed_coordinator::{SpeedCoordinator, SpeedCoordinatorError},
    speed_hotkeys::SpeedHotkeyConfig,
    speed_input::InputSnapshot,
    speed_session::SpeedAction,
};

const PREVIEW_FREQUENCY: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SpeedRequest {
    Status,
    Increase,
    Decrease,
    Toggle,
    Reset,
    SelectPreset { multiplier: f64 },
    HoldStart { multiplier: f64, duration_ms: u32 },
    HoldRelease,
    FocusLost,
}

impl SpeedRequest {
    fn action(&self) -> Option<SpeedAction> {
        match *self {
            Self::Status => None,
            Self::Increase => Some(SpeedAction::Increase),
            Self::Decrease => Some(SpeedAction::Decrease),
            Self::Toggle => Some(SpeedAction::Toggle),
            Self::Reset => Some(SpeedAction::Reset),
            Self::SelectPreset { multiplier } => Some(SpeedAction::SelectPreset(multiplier)),
            Self::HoldStart {
                multiplier,
                duration_ms,
            } => Some(SpeedAction::HoldStart {
                multiplier,
                duration_ms,
            }),
            Self::HoldRelease => Some(SpeedAction::HoldRelease),
            Self::FocusLost => Some(SpeedAction::FocusLost),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeedPreviewStatus {
    /// False means the host has not explicitly configured a shared preview.
    pub configured: bool,
    /// Descriptive origin only, not proof of DLL hook installation.
    pub profile: Option<String>,
    pub preview_multiplier: f64,
    pub focused: bool,
    pub last_action: Option<String>,
    /// Always false until a verified, separately implemented hook backend
    /// supplies authoritative applied-game-clock state.
    pub clock_applied_to_game: bool,
}

impl SpeedPreviewStatus {
    fn unavailable() -> Self {
        Self {
            configured: false,
            profile: None,
            preview_multiplier: 1.0,
            focused: false,
            last_action: None,
            clock_applied_to_game: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum SpeedPreviewError {
    #[error("speed preview is not configured for this process")]
    Unconfigured,
    #[error("speed preview state mutex is poisoned")]
    StatePoisoned,
    #[error("speed preview requires an enabled shared profile")]
    InactiveProfile,
    #[error(transparent)]
    Coordinator(#[from] SpeedCoordinatorError),
    #[error("fixture timer-hook synchronization failed: {0}")]
    FixtureClock(&'static str),
}

/// Replay validated actions into the already-installed isolated IAT
/// fixture. The backend refuses every non-fixture executable and never
/// installs hooks from this command boundary.
fn apply_fixture_action(action: SpeedAction) -> Result<(), SpeedPreviewError> {
    #[cfg(windows)]
    crate::experimental_timer_probe::apply_installed_command(action)
        .map_err(SpeedPreviewError::FixtureClock)?;
    #[cfg(not(windows))]
    let _ = action;
    Ok(())
}

/// Backend application is fallible even after preview validation. If the
/// installed clock refuses an action, disarm the preview and make one best-
/// effort request to restore the target to normal speed. Never report the
/// original action as successfully synchronized after either failure.
fn apply_with_fail_safe(
    coordinator: &mut SpeedCoordinator,
    now: u64,
    action: SpeedAction,
    mut backend: impl FnMut(SpeedAction) -> Result<(), SpeedPreviewError>,
) -> Result<(), SpeedPreviewError> {
    if let Err(error) = backend(action) {
        let _ = coordinator.apply(now, SpeedAction::FocusLost);
        let _ = backend(SpeedAction::FocusLost);
        return Err(error);
    }
    Ok(())
}

struct PreviewSession {
    coordinator: SpeedCoordinator,
    /// Profile steps used by both the preview state and an explicitly
    /// armed selected-game timer session. Never substitute default steps
    /// after profile validation.
    settings: SpeedSettings,
    started_at: Instant,
    label: String,
}

impl PreviewSession {
    fn now(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64
    }

    fn status(&self) -> SpeedPreviewStatus {
        let state = self.coordinator.status();
        SpeedPreviewStatus {
            configured: true,
            profile: Some(self.label.clone()),
            preview_multiplier: state.preview_multiplier,
            focused: state.focused,
            last_action: state.last_action.map(|action| format!("{action:?}")),
            clock_applied_to_game: false,
        }
    }
}

pub struct SpeedPreviewExecutor {
    inner: Arc<dyn CommandExecutor>,
    session: Mutex<Option<PreviewSession>>,
}

impl SpeedPreviewExecutor {
    /// Keep legacy scanner/debugger/RPC behavior unchanged when no shared
    /// speed profile is explicitly configured.
    pub fn disabled(inner: Arc<dyn CommandExecutor>) -> Self {
        Self {
            inner,
            session: Mutex::new(None),
        }
    }

    /// Explicitly opt into in-memory preview state; never interpreted as
    /// authorization to hook game timing APIs or inspect keyboard input.
    pub fn with_profile(
        inner: Arc<dyn CommandExecutor>,
        settings: &SpeedSettings,
        hotkeys: &SpeedHotkeyConfig,
        label: &str,
    ) -> Result<Self, SpeedPreviewError> {
        if !settings.enabled {
            return Err(SpeedPreviewError::InactiveProfile);
        }
        let coordinator = SpeedCoordinator::new(settings, hotkeys, 0, PREVIEW_FREQUENCY, 0, 0)?;
        Ok(Self {
            inner,
            session: Mutex::new(Some(PreviewSession {
                coordinator,
                settings: settings.clone(),
                started_at: Instant::now(),
                label: label.to_owned(),
            })),
        })
    }

    /// Snapshot the validated profile that will govern an expressly armed
    /// offline-game timer session. No profile means safe built-in steps, not
    /// permission to install hooks; the caller still performs every gate.
    #[cfg(windows)]
    pub(crate) fn selected_game_timer_settings(&self) -> Result<SpeedSettings, SpeedPreviewError> {
        let guard = self
            .session
            .lock()
            .map_err(|_| SpeedPreviewError::StatePoisoned)?;
        Ok(guard.as_ref().map_or_else(
            || SpeedSettings {
                enabled: true,
                ..SpeedSettings::default()
            },
            |session| session.settings.clone(),
        ))
    }

    /// Called only after an exact-target IAT arm succeeds. Existing explicit
    /// profile steps remain authoritative, but no startup rate is carried
    /// into the target: every arm starts at 1x.
    #[cfg(windows)]
    pub(crate) fn arm_selected_game_session(&self) -> Result<(), SpeedPreviewError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| SpeedPreviewError::StatePoisoned)?;
        if let Some(session) = guard.as_mut() {
            session
                .coordinator
                .apply(session.now(), SpeedAction::Reset)?;
        } else {
            let settings = SpeedSettings {
                enabled: true,
                ..SpeedSettings::default()
            };
            let coordinator = SpeedCoordinator::new(
                &settings,
                &SpeedHotkeyConfig::default(),
                0,
                PREVIEW_FREQUENCY,
                0,
                0,
            )?;
            *guard = Some(PreviewSession {
                coordinator,
                settings,
                started_at: Instant::now(),
                label: "explicit-offline-game".to_owned(),
            });
        }
        apply_fixture_action(SpeedAction::Reset)
    }

    pub fn sample_input(
        &self,
        snapshot: InputSnapshot,
    ) -> Result<SpeedPreviewStatus, SpeedPreviewError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| SpeedPreviewError::StatePoisoned)?;
        let session = guard.as_mut().ok_or(SpeedPreviewError::Unconfigured)?;
        let now = session.now();
        let outcome = session
            .coordinator
            .input_sample_with_actions(now, snapshot)?;
        for action in outcome.actions {
            apply_with_fail_safe(&mut session.coordinator, now, action, apply_fixture_action)?;
        }
        Ok(session.status())
    }

    /// The HWND adapter can read exactly these keys. No opt-in profile means
    /// an empty set and no keyboard sampling.
    pub fn watched_keys(&self) -> Result<Vec<u16>, SpeedPreviewError> {
        let guard = self
            .session
            .lock()
            .map_err(|_| SpeedPreviewError::StatePoisoned)?;
        Ok(guard.as_ref().map_or_else(Vec::new, |session| {
            session.coordinator.watched_keys().to_vec()
        }))
    }

    fn preview_command(&self, request: SpeedRequest) -> Result<CommandExecution, CommandError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| SpeedPreviewError::StatePoisoned)?;
        let Some(session) = guard.as_mut() else {
            if matches!(request, SpeedRequest::Status) {
                return Ok(CommandExecution {
                    result: CommandResult::Speed {
                        status: SpeedPreviewStatus::unavailable(),
                    },
                    post_action: None,
                });
            }
            return Err(SpeedPreviewError::Unconfigured.into());
        };
        let now = session.now();
        let action = request.action();
        if let Some(action) = action {
            session
                .coordinator
                .apply(now, action)
                .map_err(SpeedPreviewError::from)?;
        } else {
            let _ = session.coordinator.sample(now);
        }
        let status = session.status();
        // A status poll is observational and never re-anchors a timer lease.
        // Apply the original action to the already-installed fixture clock
        // so its hold duration expires even without a second RPC request.
        if let Some(action) = action {
            apply_with_fail_safe(&mut session.coordinator, now, action, apply_fixture_action)?;
        }
        Ok(CommandExecution {
            result: CommandResult::Speed { status },
            post_action: None,
        })
    }
}

impl CommandExecutor for SpeedPreviewExecutor {
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        match command {
            Command::Speed { request } => self.preview_command(request),
            other => self.inner.execute(other),
        }
    }

    fn shutdown(&self) {
        if let Ok(mut guard) = self.session.lock() {
            if let Some(session) = guard.as_mut() {
                let _ = session
                    .coordinator
                    .apply(session.now(), SpeedAction::FocusLost);
            }
            *guard = None;
        }
        self.inner.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speed_hotkeys::HoldHotkey;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Stub {
        shutdown: AtomicBool,
    }

    impl CommandExecutor for Stub {
        fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
            if matches!(command, Command::Ping) {
                Ok(CommandExecution {
                    result: CommandResult::Pong,
                    post_action: None,
                })
            } else {
                Err(CommandError::NotImplemented("stub"))
            }
        }

        fn shutdown(&self) {
            self.shutdown.store(true, Ordering::Release);
        }
    }

    fn stub() -> Arc<Stub> {
        Arc::new(Stub {
            shutdown: AtomicBool::new(false),
        })
    }

    fn configured(inner: Arc<dyn CommandExecutor>) -> SpeedPreviewExecutor {
        SpeedPreviewExecutor::with_profile(
            inner,
            &SpeedSettings {
                enabled: true,
                ..SpeedSettings::default()
            },
            &SpeedHotkeyConfig {
                increase: Some("Ctrl+F7".to_owned()),
                hold: Some(HoldHotkey {
                    chord: "Ctrl+H".to_owned(),
                    multiplier: 2.0,
                    duration_ms: 200,
                }),
                ..Default::default()
            },
            "explicit-shared-profile",
        )
        .unwrap()
    }

    fn report(result: CommandExecution) -> SpeedPreviewStatus {
        match result.result {
            CommandResult::Speed { status } => status,
            _ => panic!("expected speed preview result"),
        }
    }

    #[test]
    fn backend_failure_resets_preview_and_attempts_target_recovery() {
        let settings = SpeedSettings {
            enabled: true,
            ..SpeedSettings::default()
        };
        let mut coordinator = SpeedCoordinator::new(
            &settings,
            &SpeedHotkeyConfig::default(),
            0,
            PREVIEW_FREQUENCY,
            0,
            0,
        )
        .unwrap();
        coordinator
            .apply(100, SpeedAction::SelectPreset(2.0))
            .unwrap();

        let mut attempted = Vec::new();
        let failure = apply_with_fail_safe(
            &mut coordinator,
            200,
            SpeedAction::Increase,
            |action| {
                attempted.push(action);
                Err(SpeedPreviewError::FixtureClock("simulated backend failure"))
            },
        );
        assert!(matches!(failure, Err(SpeedPreviewError::FixtureClock(_))));
        assert_eq!(
            attempted,
            vec![SpeedAction::Increase, SpeedAction::FocusLost]
        );
        assert_eq!(coordinator.status().preview_multiplier, 1.0);
        assert_eq!(
            coordinator.status().last_action,
            Some(SpeedAction::FocusLost)
        );
    }

    #[test]
    fn disabled_profile_does_not_enable_preview() {
        assert!(matches!(
            SpeedPreviewExecutor::with_profile(
                stub(),
                &SpeedSettings::default(),
                &SpeedHotkeyConfig::default(),
                "disabled"
            ),
            Err(SpeedPreviewError::InactiveProfile)
        ));
    }

    #[test]
    fn unconfigured_is_read_only_and_legacy_calls_still_dispatch() {
        let executor = SpeedPreviewExecutor::disabled(stub());
        assert_eq!(
            report(
                executor
                    .execute(Command::Speed {
                        request: SpeedRequest::Status
                    })
                    .unwrap()
            ),
            SpeedPreviewStatus::unavailable()
        );
        assert!(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Increase
                })
                .is_err()
        );
        assert!(matches!(
            executor.execute(Command::Ping).unwrap().result,
            CommandResult::Pong
        ));
        assert!(executor.watched_keys().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn selected_game_timer_settings_keep_custom_steps_without_startup_acceleration() {
        let custom = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            steps: vec![0.5, 1.0, 1.5, 1.75, 2.0],
            ..SpeedSettings::default()
        };
        let executor = SpeedPreviewExecutor::with_profile(
            stub(),
            &custom,
            &SpeedHotkeyConfig::default(),
            "custom-steps",
        )
        .unwrap();
        assert_eq!(executor.selected_game_timer_settings().unwrap(), custom);
        executor.arm_selected_game_session().unwrap();
        assert_eq!(executor.selected_game_timer_settings().unwrap(), custom);
        let state = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Status,
                })
                .unwrap(),
        );
        assert_eq!(state.preview_multiplier, 1.0);
    }

    #[cfg(windows)]
    #[test]
    fn selected_game_session_initializes_safely_and_rearms_at_normal_speed() {
        let executor = SpeedPreviewExecutor::disabled(stub());
        assert!(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::SelectPreset { multiplier: 2.0 }
                })
                .is_err()
        );
        executor.arm_selected_game_session().unwrap();
        assert!(executor.watched_keys().unwrap().is_empty());
        let initial = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Status,
                })
                .unwrap(),
        );
        assert!(initial.configured);
        assert_eq!(initial.preview_multiplier, 1.0);
        let modified = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::SelectPreset { multiplier: 2.0 },
                })
                .unwrap(),
        );
        assert_eq!(modified.preview_multiplier, 2.0);
        executor.arm_selected_game_session().unwrap();
        let reset = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Status,
                })
                .unwrap(),
        );
        assert_eq!(reset.preview_multiplier, 1.0);
        assert!(!reset.clock_applied_to_game);
    }

    #[test]
    fn actions_share_state_across_frontends_without_claiming_hook_installation() {
        let executor = configured(stub());
        let first = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Increase,
                })
                .unwrap(),
        );
        assert!(first.configured);
        assert_eq!(first.preview_multiplier, 1.5);
        assert!(!first.clock_applied_to_game);
        let second = report(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Status,
                })
                .unwrap(),
        );
        assert_eq!(second.preview_multiplier, first.preview_multiplier);
        assert!(second.last_action.as_deref().unwrap().contains("Increase"));
        executor
            .sample_input(InputSnapshot {
                focused: true,
                down_keys: Vec::new(),
            })
            .unwrap();
        let focused = executor
            .sample_input(InputSnapshot {
                focused: true,
                down_keys: vec![0x11, 0x76],
            })
            .unwrap();
        assert_eq!(focused.preview_multiplier, 2.0);
        assert!(focused.focused);
        let unfocused = executor.sample_input(InputSnapshot::default()).unwrap();
        assert_eq!(unfocused.preview_multiplier, 1.0);
        assert!(!unfocused.focused);
        assert_eq!(
            report(
                executor
                    .execute(Command::Speed {
                        request: SpeedRequest::Status
                    })
                    .unwrap()
            )
            .preview_multiplier,
            1.0
        );
    }

    #[test]
    fn malformed_preset_preserves_preexisting_state_and_shutdown_forwards() {
        let inner = stub();
        let executor = configured(inner.clone());
        executor
            .execute(Command::Speed {
                request: SpeedRequest::Increase,
            })
            .unwrap();
        assert!(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::SelectPreset { multiplier: 1.25 }
                })
                .is_err()
        );
        assert_eq!(
            report(
                executor
                    .execute(Command::Speed {
                        request: SpeedRequest::Status
                    })
                    .unwrap()
            )
            .preview_multiplier,
            1.5
        );
        executor.shutdown();
        assert!(inner.shutdown.load(Ordering::Acquire));
        assert!(
            executor
                .execute(Command::Speed {
                    request: SpeedRequest::Increase
                })
                .is_err()
        );
    }

    #[test]
    fn wire_format_is_explicit_and_does_not_claim_game_speed() {
        let request: Command = serde_json::from_str(
            r#"{"command":"speed","request":{"action":"select_preset","multiplier":2.0}}"#,
        )
        .unwrap();
        assert!(matches!(
            request,
            Command::Speed {
                request: SpeedRequest::SelectPreset { multiplier: 2.0 }
            }
        ));
        let result = report(
            configured(stub())
                .execute(Command::Speed {
                    request: SpeedRequest::Status,
                })
                .unwrap(),
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"clock_applied_to_game\":false"));
    }
}
