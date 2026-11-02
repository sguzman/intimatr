//! One portable control/session boundary for speed modulation and clock state.
//!
//! Hooks, RPC, native UI and hotkeys should all act on a single instance
//! through the existing Intimatr command executor.

use thiserror::Error;

use crate::{
    speed::ClockError,
    speed_config::{SpeedProfileError, SpeedSettings},
    speed_controls::SpeedControlState,
    speed_timers::{TimerInitError, TimerSnapshot, WindowsTimers},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpeedAction {
    Increase,
    Decrease,
    Toggle,
    Reset,
    SelectPreset(f64),
}

#[derive(Debug, Error)]
pub enum SpeedSessionError {
    #[error(transparent)]
    Profile(#[from] SpeedProfileError),
    #[error(transparent)]
    Timer(#[from] TimerInitError),
    #[error(transparent)]
    Clock(#[from] ClockError),
    #[error("speed preset is not configured")]
    UnknownPreset,
}

#[derive(Debug, Clone)]
pub struct SpeedSession {
    controls: SpeedControlState,
    timers: WindowsTimers,
}

impl SpeedSession {
    pub fn new(
        settings: &SpeedSettings,
        real_qpc_origin: u64,
        qpc_frequency: u64,
        initial_tick_count64: u64,
        initial_time_get_time: u32,
    ) -> Result<Self, SpeedSessionError> {
        let controls = SpeedControlState::new(settings)?;
        let mut timers = WindowsTimers::new(
            real_qpc_origin,
            qpc_frequency,
            initial_tick_count64,
            initial_time_get_time,
        )?;
        timers.set_multiplier(real_qpc_origin, controls.effective_multiplier())?;
        Ok(Self { controls, timers })
    }

    #[must_use]
    pub fn multiplier(&self) -> f64 {
        self.timers.multiplier()
    }

    /// All API families are projected from the same real QPC sample.
    pub fn sample(&mut self, real_qpc: u64) -> TimerSnapshot {
        self.timers.sample(real_qpc)
    }

    /// Invalid presets do not modify controls or the underlying clock.
    pub fn apply(
        &mut self,
        real_qpc: u64,
        action: SpeedAction,
    ) -> Result<TimerSnapshot, SpeedSessionError> {
        let multiplier = match action {
            SpeedAction::Increase => self.controls.increase(),
            SpeedAction::Decrease => self.controls.decrease(),
            SpeedAction::Toggle => self.controls.toggle(),
            SpeedAction::Reset => self.controls.reset(),
            SpeedAction::SelectPreset(rate) => {
                if !self.controls.select_preset(rate) {
                    return Err(SpeedSessionError::UnknownPreset);
                }
                self.controls.effective_multiplier()
            }
        };
        Ok(self.timers.set_multiplier(real_qpc, multiplier)?)
    }
}

#[cfg(test)]
mod tests {
    use super::{SpeedAction, SpeedSession};
    use crate::speed_config::SpeedSettings;

    #[test]
    fn session_matches_configured_initial_speed_and_uses_shared_steps() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            ..SpeedSettings::default()
        };
        let mut session = SpeedSession::new(&settings, 0, 1_000, 10_000, 50).unwrap();
        assert_eq!(session.multiplier(), 2.0);
        assert_eq!(session.sample(500).tick_count64, 11_000);
        let at_change = session.apply(500, SpeedAction::Decrease).unwrap();
        assert_eq!(at_change.tick_count64, 11_000);
        assert_eq!(session.multiplier(), 1.5);
        assert_eq!(session.sample(1_500).tick_count64, 12_500);
    }

    #[test]
    fn toggle_reset_and_preset_remain_continuous() {
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 0, 0).unwrap();
        assert_eq!(
            session
                .apply(100, SpeedAction::Increase)
                .unwrap()
                .tick_count64,
            100
        );
        assert_eq!(session.sample(300).tick_count64, 400);
        assert_eq!(
            session
                .apply(300, SpeedAction::Toggle)
                .unwrap()
                .tick_count64,
            400
        );
        assert_eq!(session.sample(400).tick_count64, 500);
        assert_eq!(
            session
                .apply(400, SpeedAction::Toggle)
                .unwrap()
                .tick_count64,
            500
        );
        assert_eq!(session.sample(500).tick_count64, 650);
        assert_eq!(
            session.apply(500, SpeedAction::Reset).unwrap().tick_count64,
            650
        );
        assert_eq!(session.sample(600).tick_count64, 750);
        assert!(session.apply(600, SpeedAction::SelectPreset(0.5)).is_ok());
        assert_eq!(session.sample(800).tick_count64, 850);
    }

    #[test]
    fn invalid_preset_never_changes_current_clock() {
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 20, 50).unwrap();
        let before = session.sample(500);
        assert!(session.apply(700, SpeedAction::SelectPreset(1.25)).is_err());
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(500), before);
        assert_eq!(session.sample(700).tick_count64, 720);
    }

    #[test]
    fn zero_frequency_fails_instead_of_partially_initializing() {
        assert!(SpeedSession::new(&SpeedSettings::default(), 0, 0, 0, 0).is_err());
    }
}
