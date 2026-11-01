//! Platform-neutral model for advancing an in-process virtual game clock.
//!
//! A Windows timing-hook backend will translate each supported timer's native
//! units into this clock's monotonically increasing tick domain. This module
//! neither installs hooks nor modifies a process on its own.

use thiserror::Error;

/// An invalid speed must never reach a timer hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClockError {
    #[error("speed multiplier must be positive and finite")]
    InvalidMultiplier,
}

/// Virtual clock state; callers synchronize access across threads.
///
/// The first sample returns the configured virtual origin. Real samples that
/// arrive out of order are clamped to the last observed time. A speed change
/// reanchors the virtual timeline at the current sample, without a jump.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualClock {
    anchor_real: u64,
    anchor_virtual: u64,
    last_real: u64,
    last_virtual: u64,
    multiplier: f64,
}

impl VirtualClock {
    /// Create a clock with a 1× initial speed and explicit real/virtual origins.
    #[must_use]
    pub fn new(real_origin: u64, virtual_origin: u64) -> Self {
        Self {
            anchor_real: real_origin,
            anchor_virtual: virtual_origin,
            last_real: real_origin,
            last_virtual: virtual_origin,
            multiplier: 1.0,
        }
    }

    #[must_use]
    pub fn multiplier(&self) -> f64 {
        self.multiplier
    }

    /// Read the virtual time in the caller's chosen tick units.
    ///
    /// This method is nondecreasing even if callers provide an older sample.
    /// The multiplication saturates at the representable u64 boundary.
    pub fn sample(&mut self, real_ticks: u64) -> u64 {
        self.last_real = self.last_real.max(real_ticks);
        let elapsed = self.last_real.saturating_sub(self.anchor_real);
        // Rust's saturating float-to-integer conversion intentionally
        // handles values above u64::MAX without wrapping.
        let scaled = ((elapsed as f64) * self.multiplier).floor() as u64;
        let projected = self.anchor_virtual.saturating_add(scaled);
        self.last_virtual = self.last_virtual.max(projected);
        self.last_virtual
    }

    /// Change speed without discontinuity in the virtual timeline.
    ///
    /// Rejected multipliers leave the clock unchanged. Profile-specific
    /// minimum/maximum limits belong to configuration validation.
    pub fn set_multiplier(&mut self, real_ticks: u64, multiplier: f64) -> Result<u64, ClockError> {
        if !multiplier.is_finite() || multiplier <= 0.0 {
            return Err(ClockError::InvalidMultiplier);
        }

        let at_change = self.sample(real_ticks);
        self.anchor_real = self.last_real;
        self.anchor_virtual = at_change;
        self.multiplier = multiplier;
        Ok(at_change)
    }

    /// Return to 1× while preserving virtual-time continuity.
    pub fn reset(&mut self, real_ticks: u64) -> u64 {
        // 1.0 is always valid, so the result cannot fail.
        self.set_multiplier(real_ticks, 1.0)
            .expect("1.0 must be a valid speed multiplier")
    }
}

#[cfg(test)]
mod tests {
    use super::{ClockError, VirtualClock};

    #[test]
    fn starts_at_real_time_speed() {
        let mut clock = VirtualClock::new(1_000, 5_000);
        assert_eq!(clock.sample(1_000), 5_000);
        assert_eq!(clock.sample(1_100), 5_100);
    }

    #[test]
    fn slowing_and_accelerating_preserve_continuity() {
        let mut clock = VirtualClock::new(1_000, 1_000);
        assert_eq!(clock.set_multiplier(1_100, 0.25), Ok(1_100));
        assert_eq!(clock.sample(1_200), 1_125);
        assert_eq!(clock.set_multiplier(1_200, 2.0), Ok(1_125));
        assert_eq!(clock.sample(1_250), 1_225);
        assert_eq!(clock.reset(1_250), 1_225);
        assert_eq!(clock.sample(1_300), 1_275);
    }

    #[test]
    fn rejected_speeds_do_not_mutate_state() {
        let mut clock = VirtualClock::new(10, 10);
        let original = clock.clone();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                clock.set_multiplier(20, bad),
                Err(ClockError::InvalidMultiplier)
            );
            assert_eq!(clock, original);
        }
    }

    #[test]
    fn out_of_order_reads_are_monotonic() {
        let mut clock = VirtualClock::new(100, 200);
        assert_eq!(clock.sample(110), 210);
        assert_eq!(clock.sample(105), 210);
        assert_eq!(clock.set_multiplier(102, 2.0), Ok(210));
        assert_eq!(clock.sample(112), 214);
        assert_eq!(clock.sample(90), 214);
    }

    #[test]
    fn rapid_transitions_do_not_jump() {
        let mut clock = VirtualClock::new(0, 0);
        for speed in [2.0, 0.5, 3.0, 1.0, 0.125, 10.0] {
            let before = clock.sample(100);
            assert_eq!(clock.set_multiplier(100, speed), Ok(before));
            assert_eq!(clock.sample(100), before);
        }
    }

    #[test]
    fn large_elapsed_time_saturates() {
        let mut clock = VirtualClock::new(0, 5);
        assert_eq!(clock.set_multiplier(0, 10.0), Ok(5));
        assert_eq!(clock.sample(u64::MAX), u64::MAX);
        assert_eq!(clock.sample(0), u64::MAX);
    }
}
