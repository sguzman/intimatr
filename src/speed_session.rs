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
    /// A temporary configured preset with a fail-safe real-time expiry.
    HoldStart {
        multiplier: f64,
        duration_ms: u32,
    },
    HoldRelease,
    /// Losing input focus restores 1× even if the base speed was faster.
    FocusLost,
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
    #[error("hold duration must be 1..=60000 milliseconds")]
    InvalidHoldDuration,
}

#[derive(Debug, Clone)]
pub struct SpeedSession {
    controls: SpeedControlState,
    timers: WindowsTimers,
    qpc_frequency: u64,
    hold_deadline_qpc: Option<u64>,
    startup_deadline_qpc: Option<u64>,
}

/// Convert a bounded real-time interval to QPC ticks without overflow.
fn duration_qpc_ticks(qpc_frequency: u64, duration_ms: u32) -> u64 {
    (u128::from(duration_ms) * u128::from(qpc_frequency) / 1000)
        .max(1)
        .min(u128::from(u64::MAX)) as u64
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
        let startup_deadline_qpc = if settings.enabled
            && settings.initial_multiplier != 1.0
            && settings.startup_duration_ms > 0
        {
            Some(real_qpc_origin.saturating_add(duration_qpc_ticks(
                qpc_frequency,
                settings.startup_duration_ms,
            )))
        } else {
            None
        };
        Ok(Self {
            controls,
            timers,
            qpc_frequency,
            hold_deadline_qpc: None,
            startup_deadline_qpc,
        })
    }

    #[must_use]
    pub fn multiplier(&self) -> f64 {
        self.timers.multiplier()
    }

    /// All API families are projected from the same real QPC sample.
    pub fn sample(&mut self, real_qpc: u64) -> TimerSnapshot {
        self.advance_deadlines(real_qpc);
        self.timers.sample(real_qpc)
    }

    /// Apply pending deadlines in chronological order. Anchoring at their
    /// original QPC ticks prevents delayed polling from extending acceleration.
    /// Startup expiry wins a tie: it also cancels any active temporary hold.
    fn advance_deadlines(&mut self, real_qpc: u64) {
        loop {
            let (deadline, startup) = match (self.startup_deadline_qpc, self.hold_deadline_qpc) {
                (Some(startup), Some(hold)) if startup <= hold => (startup, true),
                (Some(_), Some(hold)) => (hold, false),
                (Some(startup), None) => (startup, true),
                (None, Some(hold)) => (hold, false),
                (None, None) => break,
            };
            if real_qpc < deadline {
                break;
            }
            let restored = if startup {
                self.startup_deadline_qpc = None;
                self.hold_deadline_qpc = None;
                self.controls.reset()
            } else {
                self.hold_deadline_qpc = None;
                self.controls.end_hold()
            };
            if self.timers.set_multiplier(deadline, restored).is_err() {
                // Validated speeds should make this unreachable; reset to 1×
                // rather than allowing a failed temporary clock to persist.
                self.timers.reset(deadline);
            }
        }
    }

    /// Invalid presets do not modify controls or the underlying clock.
    pub fn apply(
        &mut self,
        real_qpc: u64,
        action: SpeedAction,
    ) -> Result<TimerSnapshot, SpeedSessionError> {
        self.advance_deadlines(real_qpc);
        let mut new_deadline = None;
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
            SpeedAction::HoldStart {
                multiplier,
                duration_ms,
            } => {
                if !(1..=60_000).contains(&duration_ms) {
                    return Err(SpeedSessionError::InvalidHoldDuration);
                }
                if !self.controls.begin_hold(multiplier) {
                    return Err(SpeedSessionError::UnknownPreset);
                }
                // u128 avoids overflow even for high-resolution QPC sources.
                let ticks = duration_qpc_ticks(self.qpc_frequency, duration_ms);
                new_deadline = Some(real_qpc.saturating_add(ticks));
                self.controls.effective_multiplier()
            }
            SpeedAction::HoldRelease => self.controls.end_hold(),
            SpeedAction::FocusLost => self.controls.reset(),
        };
        let snapshot = self.timers.set_multiplier(real_qpc, multiplier)?;
        self.hold_deadline_qpc = new_deadline;
        if !matches!(
            action,
            SpeedAction::HoldStart { .. } | SpeedAction::HoldRelease
        ) {
            // Explicit persistent speed decisions supersede a startup timer.
            self.startup_deadline_qpc = None;
        }
        Ok(snapshot)
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
    fn held_speed_expires_at_deadline_without_an_extra_accelerated_interval() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 1.5,
            ..SpeedSettings::default()
        };
        let mut session = SpeedSession::new(&settings, 0, 1_000, 0, 0).unwrap();
        assert_eq!(
            session
                .apply(
                    100,
                    SpeedAction::HoldStart {
                        multiplier: 2.0,
                        duration_ms: 200,
                    },
                )
                .unwrap()
                .tick_count64,
            150
        );
        assert_eq!(session.sample(200).tick_count64, 350);
        assert_eq!(session.sample(400).tick_count64, 700);
        assert_eq!(session.multiplier(), 1.5);
        assert_eq!(session.sample(500).tick_count64, 850);
    }

    #[test]
    fn idle_status_poll_expires_hold_at_original_deadline() {
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 0, 0).unwrap();
        session
            .apply(
                100,
                SpeedAction::HoldStart {
                    multiplier: 2.0,
                    duration_ms: 100,
                },
            )
            .unwrap();
        assert_eq!(session.multiplier(), 2.0);

        // No timer callback is sampled between the hold start and this
        // simulated status poll, long after the 200-tick deadline.
        let observed = session.sample(500);
        assert_eq!(observed.tick_count64, 600);
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(600).tick_count64, 700);
    }

    #[test]
    fn release_and_focus_loss_restore_without_timer_jumps() {
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 0, 0).unwrap();
        session.apply(100, SpeedAction::Increase).unwrap();
        session
            .apply(
                200,
                SpeedAction::HoldStart {
                    multiplier: 5.0,
                    duration_ms: 1_000,
                },
            )
            .unwrap();
        assert_eq!(session.multiplier(), 5.0);
        assert_eq!(
            session
                .apply(300, SpeedAction::HoldRelease)
                .unwrap()
                .tick_count64,
            750
        );
        assert_eq!(session.multiplier(), 1.5);
        session
            .apply(
                400,
                SpeedAction::HoldStart {
                    multiplier: 3.0,
                    duration_ms: 1_000,
                },
            )
            .unwrap();
        assert_eq!(
            session
                .apply(500, SpeedAction::FocusLost)
                .unwrap()
                .tick_count64,
            1_200
        );
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(1_500).tick_count64, 2_200);
    }

    #[test]
    fn invalid_hold_input_leaves_the_clock_unchanged() {
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 0, 0).unwrap();
        for (multiplier, duration_ms) in [(2.0, 0), (2.0, 60_001), (1.25, 100)] {
            assert!(
                session
                    .apply(
                        100,
                        SpeedAction::HoldStart {
                            multiplier,
                            duration_ms,
                        },
                    )
                    .is_err()
            );
            assert_eq!(session.multiplier(), 1.0);
        }
        assert_eq!(session.sample(500).tick_count64, 500);
    }

    #[test]
    fn bounded_startup_returns_to_normal_at_the_original_deadline() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            startup_duration_ms: 200,
            ..SpeedSettings::default()
        };
        let mut session = SpeedSession::new(&settings, 0, 1_000, 0, 0).unwrap();
        assert_eq!(session.sample(100).tick_count64, 200);
        assert_eq!(session.sample(300).tick_count64, 500);
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(500).tick_count64, 700);
    }

    #[test]
    fn startup_expiry_cancels_overlapping_held_acceleration() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            startup_duration_ms: 200,
            ..SpeedSettings::default()
        };
        let mut session = SpeedSession::new(&settings, 0, 1_000, 0, 0).unwrap();
        session
            .apply(
                100,
                SpeedAction::HoldStart {
                    multiplier: 5.0,
                    duration_ms: 500,
                },
            )
            .unwrap();
        assert_eq!(session.sample(300).tick_count64, 800);
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(600).tick_count64, 1_100);
        assert_eq!(
            session
                .apply(600, SpeedAction::HoldRelease)
                .unwrap()
                .tick_count64,
            1_100
        );
    }

    #[test]
    fn explicit_persistent_action_supersedes_startup_timeout() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            startup_duration_ms: 200,
            ..SpeedSettings::default()
        };
        let mut session = SpeedSession::new(&settings, 0, 1_000, 0, 0).unwrap();
        assert_eq!(
            session
                .apply(100, SpeedAction::Increase)
                .unwrap()
                .tick_count64,
            200
        );
        assert_eq!(session.sample(400).tick_count64, 1_100);
        assert_eq!(session.multiplier(), 3.0);
    }

    #[test]
    fn zero_frequency_fails_instead_of_partially_initializing() {
        assert!(SpeedSession::new(&SpeedSettings::default(), 0, 0, 0, 0).is_err());
    }
}
