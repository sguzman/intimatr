//! One portable speed-command coordinator for future UI, RPC and focused
//! Windows input. All actions mutate one shared speed session; this module
//! does not claim the virtual clock is hooked into a running game.

use thiserror::Error;

use crate::{
    speed_config::SpeedSettings,
    speed_hotkeys::SpeedHotkeyConfig,
    speed_input::{FocusedInputPump, InputSnapshot},
    speed_session::{SpeedAction, SpeedSession, SpeedSessionError},
    speed_timers::TimerSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedCoordinatorStatus {
    /// In-memory virtual-clock speed; NOT a measurement of game-time speed.
    pub preview_multiplier: f64,
    pub focused: bool,
    pub last_action: Option<SpeedAction>,
    /// The production timing-hook backend has not been integrated here.
    pub clock_applied_to_game: bool,
}

#[derive(Debug, Error)]
pub enum SpeedCoordinatorError {
    #[error("invalid speed hotkeys: {0}")]
    Hotkeys(&'static str),
    #[error(transparent)]
    Session(#[from] SpeedSessionError),
}

pub struct SpeedCoordinator {
    session: SpeedSession,
    input: FocusedInputPump,
    focused: bool,
    last_action: Option<SpeedAction>,
}

impl SpeedCoordinator {
    pub fn new(
        settings: &SpeedSettings,
        hotkeys: &SpeedHotkeyConfig,
        real_qpc_origin: u64,
        qpc_frequency: u64,
        initial_tick_count64: u64,
        initial_time_get_time: u32,
    ) -> Result<Self, SpeedCoordinatorError> {
        let dispatcher = hotkeys
            .compile(settings)
            .map_err(SpeedCoordinatorError::Hotkeys)?;
        let input = FocusedInputPump::new(dispatcher);
        let session = SpeedSession::new(
            settings,
            real_qpc_origin,
            qpc_frequency,
            initial_tick_count64,
            initial_time_get_time,
        )?;
        Ok(Self {
            session,
            input,
            focused: false,
            last_action: None,
        })
    }

    /// Feed one verified, focused-game snapshot to the shared speed model.
    /// The input reader must have already verified HWND ownership/focus.
    pub fn input_sample(
        &mut self,
        real_qpc: u64,
        snapshot: InputSnapshot,
    ) -> Result<TimerSnapshot, SpeedCoordinatorError> {
        // Even with no input events, expire startup and held speed leases.
        self.session.sample(real_qpc);
        self.focused = snapshot.focused;
        let actions = self.input.sample(snapshot);
        for action in actions {
            if let Err(error) = self.apply(real_qpc, action) {
                self.reset_after_input_error(real_qpc);
                return Err(error);
            }
        }
        Ok(self.session.sample(real_qpc))
    }

    /// Callers controlling a shared session (future RPC, UI) use this same
    /// method; they must never construct an independent speed clock.
    pub fn apply(
        &mut self,
        real_qpc: u64,
        action: SpeedAction,
    ) -> Result<TimerSnapshot, SpeedCoordinatorError> {
        let result = self.session.apply(real_qpc, action)?;
        self.last_action = Some(action);
        Ok(result)
    }

    fn reset_after_input_error(&mut self, real_qpc: u64) {
        self.input.sample(InputSnapshot::default());
        self.focused = false;
        let _ = self.session.apply(real_qpc, SpeedAction::FocusLost);
        self.last_action = Some(SpeedAction::FocusLost);
    }

    #[must_use]
    pub fn sample(&mut self, real_qpc: u64) -> TimerSnapshot {
        self.session.sample(real_qpc)
    }

    #[must_use]
    pub fn status(&self) -> SpeedCoordinatorStatus {
        SpeedCoordinatorStatus {
            preview_multiplier: self.session.multiplier(),
            focused: self.focused,
            last_action: self.last_action,
            clock_applied_to_game: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(focused: bool, down_keys: &[u16]) -> InputSnapshot {
        InputSnapshot {
            focused,
            down_keys: down_keys.to_vec(),
        }
    }

    fn coordinator() -> SpeedCoordinator {
        SpeedCoordinator::new(
            &SpeedSettings::default(),
            &SpeedHotkeyConfig {
                increase: Some("Ctrl+F7".into()),
                hold: Some(crate::speed_hotkeys::HoldHotkey {
                    chord: "Ctrl+H".into(),
                    multiplier: 2.0,
                    duration_ms: 200,
                }),
                ..Default::default()
            },
            0,
            1_000,
            0,
            0,
        )
        .unwrap()
    }

    #[test]
    fn keyboard_and_manual_commands_share_one_clock() {
        let mut controller = coordinator();
        controller.input_sample(0, snapshot(true, &[])).unwrap();
        controller
            .input_sample(100, snapshot(true, &[0x11, 0x76]))
            .unwrap();
        assert_eq!(controller.status().preview_multiplier, 1.5);
        assert_eq!(controller.sample(200).tick_count64, 250);
        controller.apply(200, SpeedAction::Decrease).unwrap();
        assert_eq!(controller.status().preview_multiplier, 1.0);
        assert_eq!(controller.sample(300).tick_count64, 350);
        assert!(!controller.status().clock_applied_to_game);
    }

    #[test]
    fn focus_loss_restores_one_x_and_disables_stale_key_stream() {
        let mut controller = coordinator();
        controller.input_sample(0, snapshot(true, &[])).unwrap();
        controller
            .input_sample(100, snapshot(true, &[0x11, 0x48]))
            .unwrap();
        assert_eq!(controller.status().preview_multiplier, 2.0);
        controller
            .input_sample(200, snapshot(false, &[0x11, 0x48]))
            .unwrap();
        let status = controller.status();
        assert!(!status.focused);
        assert_eq!(status.preview_multiplier, 1.0);
        assert_eq!(status.last_action, Some(SpeedAction::FocusLost));
        controller
            .input_sample(300, snapshot(true, &[0x11, 0x48]))
            .unwrap();
        assert_eq!(controller.status().preview_multiplier, 1.0);
    }

    #[test]
    fn idle_sample_processes_hold_expiry_without_fresh_keys() {
        let mut controller = coordinator();
        controller.input_sample(0, snapshot(true, &[])).unwrap();
        controller
            .input_sample(100, snapshot(true, &[0x11, 0x48]))
            .unwrap();
        assert_eq!(controller.status().preview_multiplier, 2.0);
        controller
            .input_sample(600, snapshot(true, &[0x11, 0x48]))
            .unwrap();
        assert_eq!(controller.status().preview_multiplier, 1.0);
    }

    #[test]
    fn unknown_manual_preset_does_not_claim_application() {
        let mut controller = coordinator();
        assert!(controller
            .apply(100, SpeedAction::SelectPreset(1.25))
            .is_err());
        assert_eq!(controller.status().last_action, None);
        assert_eq!(controller.status().preview_multiplier, 1.0);
        assert!(!controller.status().clock_applied_to_game);
    }
}
