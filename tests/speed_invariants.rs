//! Seeded, deterministic invariant checks. Pure model only: no process hooks.
use intimatr::{speed::VirtualClock, speed_config::SharedSpeedConfig, speed_timers::WindowsTimers};
use std::{
    sync::{Arc, Mutex},
    thread,
};

#[derive(Clone, Copy)]
struct Seed(u64);
impl Seed {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
const RATES: [f64; 7] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 10.0];

#[test]
fn random_order_reads_and_25000_reanchors_do_not_jump() {
    let mut clock = VirtualClock::new(1_000, 4_000);
    let mut seed = Seed(0x6a09_e667_f3bc_c909);
    let mut real = 1_000u64;
    let mut previous = clock.sample(real);
    for i in 0..25_000 {
        let choice = seed.next();
        real = real.saturating_add(choice % 1_000);
        let observed = if choice & 7 == 0 {
            real.saturating_sub(choice % 3_000)
        } else {
            real
        };
        let before = clock.sample(observed);
        assert!(before >= previous, "backward clock at iteration {i}");
        if choice % 3 == 0 {
            let rate = RATES[(choice as usize / 3) % RATES.len()];
            assert_eq!(clock.set_multiplier(observed, rate).unwrap(), before);
        } else if choice % 29 == 0 {
            let old = clock.clone();
            for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
                assert!(clock.set_multiplier(observed, invalid).is_err());
                assert_eq!(clock, old);
            }
        } else if choice % 11 == 0 {
            assert_eq!(clock.reset(observed), before);
        }
        previous = clock.sample(observed);
        assert!(previous >= before);
    }
}

#[test]
fn all_four_timer_domains_preserve_wrap_epoch_and_monotonicity() {
    let origin64 = u64::from(u32::MAX) - 600;
    let origin32 = u32::MAX - 200;
    let mut timers = WindowsTimers::new(9_000, 10_000, origin64, origin32).unwrap();
    let mut seed = Seed(0xbb67_ae85_84ca_a73b);
    let mut real = 9_000u64;
    let mut prev = timers.sample(real);
    for i in 0..15_000 {
        let n = seed.next();
        real = real.saturating_add(n % 9_000);
        let at = if n & 15 == 0 {
            real.saturating_sub(n % 40_000)
        } else {
            real
        };
        let before = timers.sample(at);
        assert!(
            before.performance_counter >= prev.performance_counter,
            "QPC {i}"
        );
        assert!(before.tick_count64 >= prev.tick_count64, "tick64 {i}");
        if n % 5 == 0 {
            let rate = RATES[(n as usize / 5) % RATES.len()];
            assert_eq!(timers.set_multiplier(at, rate).unwrap(), before);
        } else if n % 23 == 0 {
            assert_eq!(timers.reset(at), before);
        }
        let now = timers.sample(at);
        assert!(now.performance_counter >= before.performance_counter);
        assert!(now.tick_count64 >= before.tick_count64);
        assert_eq!(now.tick_count, now.tick_count64 as u32);
        assert_eq!(
            now.time_get_time,
            origin32.wrapping_add((now.tick_count64 - origin64) as u32),
            "winmm epoch {i}"
        );
        prev = now;
    }
}

#[test]
fn six_serialized_reader_threads_preserve_clock_order_under_rate_updates() {
    let shared = Arc::new(Mutex::new((
        WindowsTimers::new(0, 10_000, 5_000, 2_000).unwrap(),
        0u64,
        5_000u64,
    )));
    let mut workers = Vec::new();
    for id in 0..6u64 {
        let shared = Arc::clone(&shared);
        workers.push(thread::spawn(move || {
            let mut seed = Seed(0x3c6e_f372_fe94_f82b ^ id);
            for i in 0..4_000 {
                let mut state = shared.lock().unwrap();
                let real = seed.next() % 500_000;
                if i % 41 == 0 {
                    let rate = RATES[(seed.next() as usize) % RATES.len()];
                    state.0.set_multiplier(real, rate).unwrap();
                }
                let now = state.0.sample(real);
                assert!(now.performance_counter >= state.1);
                assert!(now.tick_count64 >= state.2);
                state.1 = now.performance_counter;
                state.2 = now.tick_count64;
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn invalid_configurations_cannot_replace_the_last_valid_profile() {
    let valid =
        SharedSpeedConfig::parse("version = 1\n[speed]\nenabled = true\nsteps = [0.5, 1.0, 2.0]\n")
            .unwrap();
    let original = valid.clone();
    for entry in [
        "steps = [1.0, 1.0]",
        "steps = [2.0, 1.0]",
        "steps = [1.0, inf]",
        "min_multiplier = 2.0",
        "initial_multiplier = nan",
        "startup_duration_ms = 60001",
        "mispelled_option = true",
    ] {
        let text = format!("version = 1\n[speed]\n{entry}\n");
        assert!(SharedSpeedConfig::parse(&text).is_err(), "{entry}");
        assert_eq!(valid, original);
    }
    assert!(SharedSpeedConfig::parse("version = 999").is_err());
}

#[test]
fn full_width_timers_saturate_instead_of_overflow() {
    let mut clock = VirtualClock::new(u64::MAX - 3_000, u64::MAX - 500);
    clock.set_multiplier(u64::MAX - 2_000, 10.0).unwrap();
    assert_eq!(clock.sample(u64::MAX), u64::MAX);
    assert_eq!(clock.reset(u64::MAX), u64::MAX);
    let mut timers = WindowsTimers::new(0, 1, u64::MAX - 2, u32::MAX - 2).unwrap();
    timers.set_multiplier(0, 10.0).unwrap();
    let end = timers.sample(u64::MAX);
    assert_eq!(end.performance_counter, u64::MAX);
    assert_eq!(end.tick_count64, u64::MAX);
    assert_eq!(end.tick_count, u32::MAX);
}
