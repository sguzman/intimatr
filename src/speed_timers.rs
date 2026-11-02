//! Portable projection of a canonical performance counter into Windows
//! timer API domains. This does not install Windows hooks by itself.
//!
//! QPC is the canonical real-time input. GetTickCount and timeGetTime have
//! different epochs and coarser units, so callers must not feed their raw
//! counters directly to the same virtual clock.

use thiserror::Error;

use crate::speed::{ClockError, VirtualClock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TimerInitError {
    #[error("QueryPerformanceFrequency must be positive")]
    InvalidFrequency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerSnapshot {
    /// Scaled QueryPerformanceCounter ticks (QPF stays real and unchanged).
    pub performance_counter: u64,
    /// Scaled milliseconds elapsed since the origin supplied by Windows.
    pub tick_count64: u64,
    /// Lower 32 bits of tick_count64; normal 49.7-day wrap is expected.
    pub tick_count: u32,
    /// Multimedia timer in its original 32-bit millisecond domain.
    pub time_get_time: u32,
}

/// Callers serialize reads and writes to this state across target threads.
#[derive(Debug, Clone)]
pub struct WindowsTimers {
    clock: VirtualClock,
    real_qpc_origin: u64,
    initial_tick_count64: u64,
    initial_time_get_time: u32,
    qpc_frequency: u64,
}

impl WindowsTimers {
    pub fn new(
        real_qpc_origin: u64,
        qpc_frequency: u64,
        initial_tick_count64: u64,
        initial_time_get_time: u32,
    ) -> Result<Self, TimerInitError> {
        if qpc_frequency == 0 {
            return Err(TimerInitError::InvalidFrequency);
        }
        Ok(Self {
            clock: VirtualClock::new(real_qpc_origin, real_qpc_origin),
            real_qpc_origin,
            initial_tick_count64,
            initial_time_get_time,
            qpc_frequency,
        })
    }

    #[must_use]
    pub fn multiplier(&self) -> f64 {
        self.clock.multiplier()
    }

    /// Scale all API families from one sample of the real QPC.
    ///
    /// Integer widening prevents an intermediate multiplication overflow.
    /// Returned 32-bit timers intentionally preserve Windows wraparound.
    pub fn sample(&mut self, real_qpc: u64) -> TimerSnapshot {
        let virtual_qpc = self.clock.sample(real_qpc);
        let scaled_delta = virtual_qpc.saturating_sub(self.real_qpc_origin);
        let elapsed_ms = ((u128::from(scaled_delta) * 1000) / u128::from(self.qpc_frequency))
            .min(u128::from(u64::MAX)) as u64;

        let tick_count64 = self.initial_tick_count64.saturating_add(elapsed_ms);
        TimerSnapshot {
            performance_counter: virtual_qpc,
            tick_count64,
            tick_count: tick_count64 as u32,
            time_get_time: self.initial_time_get_time.wrapping_add(elapsed_ms as u32),
        }
    }

    /// Reanchor all timer domains at one speed change, with no time jump.
    pub fn set_multiplier(
        &mut self,
        real_qpc: u64,
        multiplier: f64,
    ) -> Result<TimerSnapshot, ClockError> {
        self.clock.set_multiplier(real_qpc, multiplier)?;
        Ok(self.sample(real_qpc))
    }

    /// Reset to real-time rate, without resetting elapsed virtual time.
    pub fn reset(&mut self, real_qpc: u64) -> TimerSnapshot {
        self.clock.reset(real_qpc);
        self.sample(real_qpc)
    }
}

#[cfg(test)]
mod tests {
    use super::{TimerInitError, WindowsTimers};

    #[test]
    fn rejects_zero_performance_frequency() {
        assert_eq!(
            WindowsTimers::new(100, 0, 0, 0).unwrap_err(),
            TimerInitError::InvalidFrequency
        );
    }

    #[test]
    fn one_x_qpc_and_milliseconds_are_consistent() {
        let mut timers = WindowsTimers::new(10_000, 10_000, 42_000, 500).unwrap();
        let first = timers.sample(10_000);
        assert_eq!(first.performance_counter, 10_000);
        assert_eq!(first.tick_count64, 42_000);
        assert_eq!(first.time_get_time, 500);

        let later = timers.sample(15_000);
        assert_eq!(later.performance_counter, 15_000);
        assert_eq!(later.tick_count64, 42_500);
        assert_eq!(later.tick_count, 42_500);
        assert_eq!(later.time_get_time, 1_000);
    }

    #[test]
    fn rate_changes_affect_every_timer_without_jumps() {
        let mut timers = WindowsTimers::new(10_000, 10_000, 100_000, 200).unwrap();
        let before = timers.sample(20_000);
        assert_eq!(before.tick_count64, 101_000);

        let changed = timers.set_multiplier(20_000, 0.5).unwrap();
        assert_eq!(changed, before);
        let slow = timers.sample(30_000);
        assert_eq!(slow.performance_counter, 25_000);
        assert_eq!(slow.tick_count64, 101_500);
        assert_eq!(slow.time_get_time, 1_700);

        let changed_again = timers.set_multiplier(30_000, 2.0).unwrap();
        assert_eq!(changed_again, slow);
        let fast = timers.sample(40_000);
        assert_eq!(fast.performance_counter, 45_000);
        assert_eq!(fast.tick_count64, 103_500);
        assert_eq!(fast.time_get_time, 3_700);
    }

    #[test]
    fn stale_samples_and_invalid_speed_cannot_move_time_backward() {
        let mut timers = WindowsTimers::new(1_000, 1_000, 1_000, 50).unwrap();
        let current = timers.sample(2_000);
        assert_eq!(timers.sample(1_500), current);
        assert!(timers.set_multiplier(3_000, 0.0).is_err());
        assert_eq!(timers.multiplier(), 1.0);
        assert_eq!(timers.sample(2_000), current);
        assert_eq!(timers.reset(2_000), current);
    }

    #[test]
    fn full_width_clock_stays_monotonic_while_32_bit_clock_wraps() {
        let mut timers =
            WindowsTimers::new(0, 1_000, u64::from(u32::MAX) - 4, u32::MAX - 4).unwrap();
        let after = timers.sample(10);
        assert_eq!(after.tick_count64, u64::from(u32::MAX) + 6);
        assert_eq!(after.tick_count, 5);
        assert_eq!(after.time_get_time, 5);
    }

    #[test]
    fn saturates_at_large_values_without_panicking() {
        let mut timers = WindowsTimers::new(0, 1, u64::MAX - 1, 10).unwrap();
        timers.set_multiplier(0, 10.0).unwrap();
        assert_eq!(timers.sample(u64::MAX).tick_count64, u64::MAX);
    }
}
