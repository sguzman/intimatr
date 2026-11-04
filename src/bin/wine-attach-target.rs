//! Disposable offline Wine target for explicit running-process DLL loading.
//! The native Linux CI job starts this process *before* the helper.
//! It never reads or alters a third-party game.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-attach-target must run as a Windows PE executable");
    std::process::exit(2);
}

#[cfg(windows)]
mod target {
    use std::{
        ffi::c_void,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn GetTickCount64() -> u64;
        fn GetTickCount() -> u32;
    }

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeGetTime() -> u32;
    }

    type StateFn = unsafe extern "system" fn() -> u32;
    type ShutdownFn = unsafe extern "system" fn() -> i32;
    type ToggleFn = unsafe extern "system" fn() -> i32;
    type RateFn = unsafe extern "system" fn(f64) -> i32;
    type CountFn = unsafe extern "system" fn() -> u32;
    type QpcFn = unsafe extern "system" fn(*mut i64) -> i32;
    type Timer64Fn = unsafe extern "system" fn() -> u64;
    type Timer32Fn = unsafe extern "system" fn() -> u32;

    fn resolve(module: *mut c_void, name: &std::ffi::CStr) -> Result<*const c_void, String> {
        let result = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        if result.is_null() {
            Err(format!(
                "missing required DLL export: {}",
                name.to_string_lossy()
            ))
        } else {
            Ok(result)
        }
    }

    fn read_qpc(function: QpcFn) -> Result<i64, String> {
        let mut counter = 0_i64;
        if unsafe { function(&mut counter) } == 0 {
            return Err("QPC read failed in remote attach fixture".into());
        }
        Ok(counter)
    }

    /// Check all four imported clocks against their original unhooked entry
    /// points in the target process itself; no helper process does the reads.
    fn check_scaled_clocks(rate: f64) -> Result<(), String> {
        let kernel_name: &[u16] = &[107, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0];
        let winmm_name: &[u16] = &[119, 105, 110, 109, 109, 46, 100, 108, 108, 0];
        let kernel = unsafe { GetModuleHandleW(kernel_name.as_ptr()) };
        let winmm = unsafe { GetModuleHandleW(winmm_name.as_ptr()) };
        if kernel.is_null() || winmm.is_null() {
            return Err("could not locate original timer modules".into());
        }
        let real_qpc: QpcFn =
            unsafe { std::mem::transmute(resolve(kernel, c"QueryPerformanceCounter")?) };
        let real_tick64: Timer64Fn =
            unsafe { std::mem::transmute(resolve(kernel, c"GetTickCount64")?) };
        let real_tick32: Timer32Fn =
            unsafe { std::mem::transmute(resolve(kernel, c"GetTickCount")?) };
        let real_winmm: Timer32Fn = unsafe { std::mem::transmute(resolve(winmm, c"timeGetTime")?) };
        let hooked_qpc: QpcFn = QueryPerformanceCounter;
        let vq1 = read_qpc(hooked_qpc)?;
        let rq1 = read_qpc(real_qpc)?;
        let v64_1 = unsafe { GetTickCount64() };
        let r64_1 = unsafe { real_tick64() };
        let v32_1 = unsafe { GetTickCount() };
        let r32_1 = unsafe { real_tick32() };
        let vw_1 = unsafe { timeGetTime() };
        let rw_1 = unsafe { real_winmm() };

        thread::sleep(Duration::from_millis(175));

        let vq2 = read_qpc(hooked_qpc)?;
        let rq2 = read_qpc(real_qpc)?;
        let v64_2 = unsafe { GetTickCount64() };
        let r64_2 = unsafe { real_tick64() };
        let v32_2 = unsafe { GetTickCount() };
        let r32_2 = unsafe { real_tick32() };
        let vw_2 = unsafe { timeGetTime() };
        let rw_2 = unsafe { real_winmm() };

        let real_q = rq2 - rq1;
        let real_64 = r64_2.saturating_sub(r64_1);
        let real_32 = r32_2.wrapping_sub(r32_1);
        let real_w = rw_2.wrapping_sub(rw_1);
        if real_q <= 0 || real_64 == 0 || real_32 == 0 || real_w == 0 {
            return Err("unhooked baseline did not advance".into());
        }
        let observed = [
            (vq2 - vq1) as f64 / real_q as f64,
            v64_2.saturating_sub(v64_1) as f64 / real_64 as f64,
            v32_2.wrapping_sub(v32_1) as f64 / real_32 as f64,
            vw_2.wrapping_sub(vw_1) as f64 / real_w as f64,
        ];
        println!(
            "Remote-attached Wine clocks requested={rate:.2}x qpc={:.3}x tick64={:.3}x tick32={:.3}x winmm={:.3}x",
            observed[0], observed[1], observed[2], observed[3]
        );
        if observed
            .iter()
            .any(|measured| (measured - rate).abs() > 0.30)
        {
            return Err(format!(
                "remote-attached clock ratios not within tolerance at {rate}x"
            ));
        }
        Ok(())
    }

    fn exercise_remote_speed(module: *mut c_void, directory: &Path) -> Result<(), String> {
        let enable: ToggleFn =
            unsafe { std::mem::transmute(resolve(module, c"intimatr_timer_probe_enable")?) };
        let disable: ToggleFn =
            unsafe { std::mem::transmute(resolve(module, c"intimatr_timer_probe_disable")?) };
        let count: CountFn =
            unsafe { std::mem::transmute(resolve(module, c"intimatr_timer_probe_hook_count")?) };
        let change: RateFn =
            unsafe { std::mem::transmute(resolve(module, c"intimatr_timer_probe_rate")?) };
        if std::env::var_os("INTIMATR_TEST_EXTERNAL_TIMER_ARM").is_some() {
            let ready = directory.join("attach-await-arm.flag");
            let released = directory.join("attach-external-armed.flag");
            std::fs::write(&ready, b"awaiting_explicit_remote_arm")
                .map_err(|error| format!("cannot publish external arm gate: {error}"))?;
            if !(0..400).any(|_| {
                if released.is_file() {
                    true
                } else {
                    thread::sleep(Duration::from_millis(25));
                    false
                }
            }) {
                return Err("test coordinator did not explicitly arm timers".into());
            }
        } else if unsafe { enable() } == 0 {
            return Err("remote-attached target could not activate four timer hooks".into());
        }
        let measured = (|| {
            if unsafe { count() } != 4 {
                return Err(format!(
                    "remote-attached target expected exactly four timer imports, observed {}",
                    unsafe { count() }
                ));
            }
            // Only the explicitly flagged CI game fixture pauses with hooks
            // installed for external status inspection; no normal target waits.
            if std::env::var_os("INTIMATR_TEST_HOOK_STATUS_GATE").is_some() {
                let ready = directory.join("attach-hooks-ready.flag");
                let release = directory.join("attach-hooks-inspected.flag");
                std::fs::write(&ready, b"four_hooks_installed")
                    .map_err(|error| format!("cannot publish hook status gate: {error}"))?;
                if !(0..400).any(|_| {
                    if release.is_file() {
                        true
                    } else {
                        thread::sleep(Duration::from_millis(25));
                        false
                    }
                }) {
                    return Err("test coordinator did not inspect installed timer hooks".into());
                }
            }
            for (speed, label) in [
                (0.5, "half"),
                (1.5, "onehalf"),
                (1.75, "custom"),
                (1.001, "thousandth"),
                (2.0, "double"),
                (1.0, "normal"),
            ] {
                // Non-default 1.75x and 1.001x steps exist only in the
                // explicit selected-game profile. The ordinary attach fixture
                // must never try to select either via its built-in steps.
                if matches!(label, "custom" | "thousandth")
                    && std::env::var_os("INTIMATR_TEST_EXTERNAL_RATE_COMMANDS").is_none()
                {
                    continue;
                }
                if std::env::var_os("INTIMATR_TEST_EXTERNAL_RATE_COMMANDS").is_some() {
                    let ready = directory.join(format!("attach-rate-{label}-ready.flag"));
                    let done = directory.join(format!("attach-rate-{label}-done.flag"));
                    std::fs::write(&ready, b"awaiting_explicit_speed_preset")
                        .map_err(|error| format!("cannot publish rate gate: {error}"))?;
                    if !(0..800).any(|_| {
                        if done.is_file() {
                            true
                        } else {
                            thread::sleep(Duration::from_millis(25));
                            false
                        }
                    }) {
                        return Err(format!("test coordinator did not set {label} rate"));
                    }
                } else if unsafe { change(speed) } == 0 {
                    return Err(format!(
                        "remote-attached target rejected valid speed {speed}"
                    ));
                }
                check_scaled_clocks(speed)?;
            }
            Ok(())
        })();
        // In the selected-game CI case the *external* stop-game command is
        // responsible for restoring these still-installed imports. The
        // ordinary attach fixture continues to exercise its own cleanup.
        if std::env::var_os("INTIMATR_TEST_REMOTE_STOP_WITH_HOOKS").is_some() {
            if measured.is_ok()
                && std::env::var_os("INTIMATR_TEST_EXTERNAL_RATE_COMMANDS").is_none()
                && unsafe { change(2.0) } == 0
            {
                return Err("remote-stop fixture could not retain a 2x timer rate".into());
            }
            return measured;
        }
        let disabled = unsafe { disable() } != 0 && unsafe { count() } == 0;
        if !disabled {
            return Err("remote-attached target failed to restore timing imports".into());
        }
        measured
    }

    pub fn run() -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let directory = executable.parent().unwrap_or_else(|| Path::new("."));
        // Opt-in CI-only observation of the environment received by this
        // *Windows process*, rather than merely by the Linux launch CLI.
        // Never inspect arbitrary games or log their environment.
        if std::env::var_os("INTIMATR_TEST_REPORT_PROFILE_AT_START").is_some() {
            let observed = std::env::var("INTIMATR_SPEED_PROFILE")
                .map_err(|error| format!("missing Windows launch speed profile: {error}"))?;
            std::fs::write(directory.join("attach-profile-observed.txt"), observed)
                .map_err(|error| format!("cannot publish Wine child profile receipt: {error}"))?;
        }
        let pid_file = directory.join("attach-target.pid");
        std::fs::write(&pid_file, std::process::id().to_string())
            .map_err(|error| format!("cannot write target PID: {error}"))?;
        println!(
            "Offline Wine attach target awaiting DLL; pid={}",
            std::process::id()
        );

        let wide: Vec<u16> = "intimatr.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        for _ in 0..800 {
            let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
            if !module.is_null() {
                let state =
                    unsafe { GetProcAddress(module, c"intimatr_lifecycle_state".as_ptr().cast()) };
                let shutdown =
                    unsafe { GetProcAddress(module, c"intimatr_request_shutdown".as_ptr().cast()) };
                if state.is_null() || shutdown.is_null() {
                    return Err("remote-loaded DLL is missing lifecycle exports".into());
                }
                let state: StateFn = unsafe { std::mem::transmute(state) };
                let stop: ShutdownFn = unsafe { std::mem::transmute(shutdown) };
                for _ in 0..200 {
                    match unsafe { state() } {
                        3 => {
                            // CI-only fail-closed fixture for a supplied but
                            // disabled launch profile. The target stays alive
                            // until the separately selected status/arm/stop
                            // commands have independently inspected it.
                            if std::env::var_os("INTIMATR_TEST_REJECTED_PROFILE_ONLY").is_some() {
                                type ProfileStateFn = unsafe extern "system" fn(*mut c_void) -> u32;
                                let check: ProfileStateFn = unsafe {
                                    std::mem::transmute(resolve(
                                        module,
                                        c"intimatr_remote_speed_profile_state",
                                    )?)
                                };
                                if unsafe { check(std::ptr::null_mut()) } != 2 {
                                    return Err(
                                        "disabled launch profile was not rejected by target".into(),
                                    );
                                }
                                std::fs::write(
                                    directory.join("attach-rejected-profile-ready.flag"),
                                    b"explicit_disabled_profile_rejected",
                                )
                                .map_err(|error| {
                                    format!("cannot publish rejected-profile readiness: {error}")
                                })?;
                                for _ in 0..1600 {
                                    if unsafe { state() } == 5 {
                                        println!(
                                            "Wine target rejected disabled profile and observed clean external stop"
                                        );
                                        return Ok(());
                                    }
                                    thread::sleep(Duration::from_millis(25));
                                }
                                return Err(
                                    "rejected-profile test coordinator did not stop target".into(),
                                );
                            }
                            // Allow the remote helper to finish its load thread
                            // before this deliberately controlled target exits.
                            thread::sleep(Duration::from_millis(250));
                            exercise_remote_speed(module, directory)?;
                            // In the external-stop regression, keep two threads
                            // reading the hooked timing APIs during the remote
                            // shutdown. The DLL must restore imports while
                            // callbacks may still be executing and stay mapped.
                            let timer_stress =
                                (std::env::var_os("INTIMATR_TEST_REMOTE_STOP_WITH_HOOKS")
                                    .is_some())
                                .then(|| {
                                    let live = Arc::new(AtomicBool::new(true));
                                    let threads = (0..2)
                                        .map(|_| {
                                            let live = Arc::clone(&live);
                                            thread::spawn(move || {
                                                while live.load(Ordering::Acquire) {
                                                    let mut qpc = 0_i64;
                                                    assert_ne!(
                                                        unsafe {
                                                            QueryPerformanceCounter(&mut qpc)
                                                        },
                                                        0
                                                    );
                                                    let ticks = unsafe {
                                                        (
                                                            GetTickCount64(),
                                                            GetTickCount(),
                                                            timeGetTime(),
                                                        )
                                                    };
                                                    std::hint::black_box((qpc, ticks));
                                                    thread::sleep(Duration::from_millis(1));
                                                }
                                            })
                                        })
                                        .collect::<Vec<_>>();
                                    (live, threads)
                                });
                            // Keep the disposable process alive long enough to
                            // assert that a second explicit attach is refused.
                            let verified = directory.join("attach-verified.flag");
                            let release = directory.join("attach-release.flag");
                            std::fs::write(&verified, b"four_timers_verified").map_err(
                                |error| format!("cannot publish attach status: {error}"),
                            )?;
                            let mut released = false;
                            for _ in 0..400 {
                                if release.is_file() {
                                    released = true;
                                    break;
                                }
                                thread::sleep(Duration::from_millis(25));
                            }
                            if !released {
                                return Err("test coordinator did not release attach target".into());
                            }
                            if unsafe { stop() } == 0 || unsafe { state() } != 5 {
                                return Err("attached DLL failed clean shutdown".into());
                            }
                            if let Some((live, threads)) = timer_stress {
                                live.store(false, Ordering::Release);
                                for worker in threads {
                                    worker.join().map_err(
                                        |_| "timer worker panicked during external stop",
                                    )?;
                                }
                            }
                            println!("Wine attach target verified 0.5x/2x/1x timers, then Stopped");
                            return Ok(());
                        }
                        6 => return Err("attached DLL reported failed bootstrap".into()),
                        _ => thread::sleep(Duration::from_millis(20)),
                    }
                }
                return Err("attached DLL never reached Running".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err("timed out without receiving the test DLL".into())
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = target::run() {
        eprintln!("Wine attach target: {error}");
        std::process::exit(1);
    }
}
