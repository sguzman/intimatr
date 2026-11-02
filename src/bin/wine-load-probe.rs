//! Small unprotected Windows process used to smoke-test Wine DLL loading.
//! It does not inject into any other process, scan memory, or alter game files.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-load-probe is a Windows executable built on a Linux host");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows_probe {
    use std::{
        ffi::{CStr, c_void},
        fs::OpenOptions,
        io::Write,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(path: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetModuleHandleW(module: *const u16) -> *mut c_void;
    }

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeGetTime() -> u32;
    }

    /// Disposable fixture configuration, created exclusively in the Wine
    /// test process. Never touches any user's shared profile or Bottles.
    struct TemporaryPreviewProfile(PathBuf);

    impl TemporaryPreviewProfile {
        fn create() -> Result<Self, String> {
            let path = std::env::temp_dir().join(format!(
                "intimatr-fixture-speed-{}-{}.toml",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_nanos()
            ));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("cannot create disposable speed profile: {error}"))?;
            file.write_all(b"version = 1\n[speed]\nenabled = true\n")
                .map_err(|error| format!("cannot write disposable speed profile: {error}"))?;
            // SAFETY: before LoadLibrary/bootstrap and before this fixture
            // spawns any threads; no other process's environment is changed.
            unsafe { std::env::set_var("INTIMATR_SPEED_PROFILE", &path) };
            Ok(Self(path))
        }
    }

    impl Drop for TemporaryPreviewProfile {
        fn drop(&mut self) {
            // The disposable test file is ours, created with create_new.
            let _ = std::fs::remove_file(&self.0);
        }
    }

    struct LoadedModule(*mut c_void);

    impl Drop for LoadedModule {
        fn drop(&mut self) {
            // Errors while a hook is installed must never unload a DLL whose
            // callbacks could still be referenced by this executable.
            let disable =
                unsafe { GetProcAddress(self.0, c"intimatr_timer_probe_disable".as_ptr().cast()) };
            let count = unsafe {
                GetProcAddress(self.0, c"intimatr_timer_probe_hook_count".as_ptr().cast())
            };
            if !disable.is_null() && !count.is_null() {
                let stop: BoolFn = unsafe { std::mem::transmute(disable) };
                let active: unsafe extern "system" fn() -> u32 =
                    unsafe { std::mem::transmute(count) };
                if unsafe { stop() } == 0 || unsafe { active() } != 0 {
                    eprintln!("Wine probe retaining loaded DLL: unsafe hook restoration state");
                    return;
                }
            }
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }

    type StateFn = unsafe extern "system" fn() -> u32;
    type BoolFn = unsafe extern "system" fn() -> i32;
    type RateFn = unsafe extern "system" fn(f64) -> i32;
    type QpcFn = unsafe extern "system" fn(*mut i64) -> i32;
    type Tick64Fn = unsafe extern "system" fn() -> u64;
    type Tick32Fn = unsafe extern "system" fn() -> u32;
    type TimeGetTimeFn = unsafe extern "system" fn() -> u32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn GetTickCount64() -> u64;
        fn GetTickCount() -> u32;
    }

    unsafe fn imported_qpc() -> Result<i64, String> {
        let mut value = 0_i64;
        if unsafe { QueryPerformanceCounter(&mut value) } == 0 {
            return Err("imported QPC returned failure".to_owned());
        }
        Ok(value)
    }

    fn kernel32_module() -> Result<*mut c_void, String> {
        let name: &[u16] = &[107, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0];
        let kernel = unsafe { GetModuleHandleW(name.as_ptr()) };
        if kernel.is_null() {
            Err("cannot find kernel32 module".to_owned())
        } else {
            Ok(kernel)
        }
    }

    unsafe fn original_qpc() -> Result<i64, String> {
        let kernel = kernel32_module()?;
        let address = unsafe { GetProcAddress(kernel, c"QueryPerformanceCounter".as_ptr().cast()) };
        if address.is_null() {
            return Err("cannot resolve original QPC".to_owned());
        }
        let real: QpcFn = unsafe { std::mem::transmute(address) };
        let mut value = 0_i64;
        if unsafe { real(&mut value) } == 0 {
            return Err("original QPC returned failure".to_owned());
        }
        Ok(value)
    }

    unsafe fn original_tick64() -> Result<u64, String> {
        let kernel = kernel32_module()?;
        let address = unsafe { GetProcAddress(kernel, c"GetTickCount64".as_ptr().cast()) };
        if address.is_null() {
            return Err("cannot resolve unmodified GetTickCount64".to_owned());
        }
        let real: Tick64Fn = unsafe { std::mem::transmute(address) };
        Ok(unsafe { real() })
    }

    unsafe fn original_tick32() -> Result<u32, String> {
        let kernel = kernel32_module()?;
        let address = unsafe { GetProcAddress(kernel, c"GetTickCount".as_ptr().cast()) };
        if address.is_null() {
            return Err("cannot resolve unmodified GetTickCount".to_owned());
        }
        let real: Tick32Fn = unsafe { std::mem::transmute(address) };
        Ok(unsafe { real() })
    }

    unsafe fn original_time_get_time() -> Result<u32, String> {
        let module_name: &[u16] = &[119, 105, 110, 109, 109, 46, 100, 108, 108, 0];
        let winmm = unsafe { GetModuleHandleW(module_name.as_ptr()) };
        if winmm.is_null() {
            return Err("cannot find winmm module".to_owned());
        }
        let address = unsafe { GetProcAddress(winmm, c"timeGetTime".as_ptr().cast()) };
        if address.is_null() {
            return Err("cannot resolve unmodified timeGetTime".to_owned());
        }
        let original: TimeGetTimeFn = unsafe { std::mem::transmute(address) };
        Ok(unsafe { original() })
    }

    fn measure_millisecond_timers(rate: f64) -> Result<(), String> {
        let virtual64_before = unsafe { GetTickCount64() };
        let real64_before = unsafe { original_tick64()? };
        let virtual32_before = unsafe { GetTickCount() };
        let real32_before = unsafe { original_tick32()? };
        let virtual_winmm_before = unsafe { timeGetTime() };
        let real_winmm_before = unsafe { original_time_get_time()? };
        thread::sleep(Duration::from_millis(160));
        let virtual64_after = unsafe { GetTickCount64() };
        let real64_after = unsafe { original_tick64()? };
        let virtual32_after = unsafe { GetTickCount() };
        let real32_after = unsafe { original_tick32()? };
        let virtual_winmm_after = unsafe { timeGetTime() };
        let real_winmm_after = unsafe { original_time_get_time()? };
        let real64_delta = real64_after.saturating_sub(real64_before);
        let real32_delta = real32_after.wrapping_sub(real32_before);
        let real_winmm_delta = real_winmm_after.wrapping_sub(real_winmm_before);
        if real64_delta == 0 || real32_delta == 0 || real_winmm_delta == 0 {
            return Err("unmodified Windows millisecond timers did not advance".to_owned());
        }
        let ratio64 = virtual64_after.saturating_sub(virtual64_before) as f64 / real64_delta as f64;
        let ratio32 = virtual32_after.wrapping_sub(virtual32_before) as f64 / real32_delta as f64;
        let ratio_winmm =
            virtual_winmm_after.wrapping_sub(virtual_winmm_before) as f64 / real_winmm_delta as f64;
        println!(
            "Wine ms timer hooks: requested={rate:.2}x tick64={ratio64:.3}x tick32={ratio32:.3}x winmm={ratio_winmm:.3}x"
        );
        if (ratio64 - rate).abs() > 0.30
            || (ratio32 - rate).abs() > 0.30
            || (ratio_winmm - rate).abs() > 0.30
        {
            return Err(format!(
                "inconsistent Windows millisecond timer scaling at {rate}x"
            ));
        }
        Ok(())
    }

    fn measure_rate(rate: f64, set_rate: RateFn) -> Result<(), String> {
        if unsafe { set_rate(rate) } == 0 {
            return Err(format!("could not select speed preset {rate}"));
        }
        measure_current_rate(rate)
    }

    fn measure_current_rate(rate: f64) -> Result<(), String> {
        let virtual_before = unsafe { imported_qpc()? };
        let real_before = unsafe { original_qpc()? };
        thread::sleep(Duration::from_millis(150));
        let virtual_after = unsafe { imported_qpc()? };
        let real_after = unsafe { original_qpc()? };
        let real_elapsed = real_after - real_before;
        if real_elapsed <= 0 {
            return Err("real QPC elapsed time did not advance".to_owned());
        }
        let observed = (virtual_after - virtual_before) as f64 / real_elapsed as f64;
        println!("Wine QPC hook: requested={rate:.2}x observed={observed:.3}x");
        if (observed - rate).abs() > 0.25 {
            return Err(format!(
                "unexpected QPC speed: expected {rate}, got {observed}"
            ));
        }
        measure_millisecond_timers(rate)?;
        Ok(())
    }
    fn stress_concurrent_timers(set_rate: RateFn) -> Result<(), String> {
        let threads = 4_usize;
        let gate = Arc::new(Barrier::new(threads + 1));
        let mut workers = Vec::new();
        for _ in 0..threads {
            let start = Arc::clone(&gate);
            workers.push(thread::spawn(move || -> Result<(), String> {
                start.wait();
                let mut last_qpc = unsafe { imported_qpc()? };
                let mut last_tick64 = unsafe { GetTickCount64() };
                let mut last_tick32 = unsafe { GetTickCount() };
                let mut last_winmm = unsafe { timeGetTime() };
                for _ in 0..15_000 {
                    let qpc = unsafe { imported_qpc()? };
                    let tick64 = unsafe { GetTickCount64() };
                    let tick32 = unsafe { GetTickCount() };
                    let winmm = unsafe { timeGetTime() };
                    if qpc < last_qpc
                        || tick64 < last_tick64
                        || tick32 < last_tick32
                        || winmm < last_winmm
                    {
                        return Err("one of the four virtual timer APIs moved backward".to_owned());
                    }
                    last_qpc = qpc;
                    last_tick64 = tick64;
                    last_tick32 = tick32;
                    last_winmm = winmm;
                }
                Ok(())
            }));
        }
        gate.wait();
        for i in 0..30 {
            let rate = if i % 2 == 0 { 0.5 } else { 2.0 };
            if unsafe { set_rate(rate) } == 0 {
                return Err("could not change speed during concurrent timer reads".to_owned());
            }
            thread::sleep(Duration::from_millis(1));
        }
        for worker in workers {
            worker
                .join()
                .map_err(|_| "timer stress worker panicked".to_owned())??;
        }
        if unsafe { set_rate(1.0) } == 0 {
            return Err("could not reset speed after concurrency test".to_owned());
        }
        println!(
            "Wine timer concurrency: 4 workers x 15000 samples, 30 rate updates, no backward jumps"
        );
        Ok(())
    }

    type ShutdownFn = unsafe extern "system" fn() -> i32;

    unsafe fn resolve(module: &LoadedModule, name: &'static CStr) -> Result<*const c_void, String> {
        let address = unsafe { GetProcAddress(module.0, name.as_ptr().cast()) };
        if address.is_null() {
            return Err(format!(
                "missing exported function {:?}",
                name.to_string_lossy()
            ));
        }
        Ok(address)
    }

    /// Validate the actual Windows CLI through the DLL's local RPC server.
    /// This fixture-only test never attaches to another process or changes
    /// a real game clock; no global keyboard or timing hook is installed.
    fn exercise_speed_cli(directory: &Path) -> Result<(), String> {
        fn command(client: &Path, request: &[&str]) -> Result<serde_json::Value, String> {
            let output = std::process::Command::new(client)
                .args(request)
                .output()
                .map_err(|error| format!("cannot spawn speed RPC client: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "speed RPC client failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid CLI speed response JSON: {error}"))
        }

        let client = directory.join("intimatr-speed.exe");
        if !client.is_file() {
            return Err(format!(
                "isolated Windows RPC client not found: {}",
                client.display()
            ));
        }
        let first = command(&client, &["--tcp", "127.0.0.1:31379", "status"])?;
        if first["configured"] != true
            || first["preview_multiplier"] != 1.0
            || first["clock_applied_to_game"] != false
        {
            return Err(format!("unexpected initial shared preview: {first}"));
        }
        let increased = command(&client, &["--tcp", "127.0.0.1:31379", "increase"])?;
        if increased["preview_multiplier"] != 1.5 || increased["clock_applied_to_game"] != false {
            return Err(format!(
                "shared RPC step did not update preview: {increased}"
            ));
        }
        let reset = command(&client, &["--tcp", "127.0.0.1:31379", "reset"])?;
        if reset["preview_multiplier"] != 1.0 {
            return Err(format!("shared RPC reset did not restore 1x: {reset}"));
        }
        let denied = std::process::Command::new(&client)
            .args(["--tcp", "192.168.1.55:31379", "status"])
            .output()
            .map_err(|error| format!("cannot check remote endpoint refusal: {error}"))?;
        if denied.status.success() {
            return Err("speed CLI unexpectedly contacted a non-loopback endpoint".into());
        }
        println!("Wine speed RPC CLI roundtrip and loopback-only policy verified");
        Ok(())
    }

    /// This verifies the *normal external RPC client*, rather than the
    /// special exported probe-rate setter, controls real imported timer
    /// reads inside the already-hooked disposable Wine fixture.
    fn exercise_live_hook_through_rpc(directory: &Path) -> Result<(), String> {
        let client = directory.join("intimatr-speed.exe");
        for (action, value, expected) in [
            ("preset", Some("2.0"), 2.0),
            ("reset", None, 1.0),
            ("preset", Some("0.5"), 0.5),
            ("reset", None, 1.0),
        ] {
            let mut command = std::process::Command::new(&client);
            command.args(["--tcp", "127.0.0.1:31379", action]);
            if let Some(value) = value {
                command.arg(value);
            }
            let output = command
                .output()
                .map_err(|error| format!("cannot invoke speed RPC client with hooks: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "speed RPC command rejected while fixture hooked: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let status: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("bad speed RPC response: {error}"))?;
            if status["preview_multiplier"] != expected || status["clock_applied_to_game"] != false
            {
                return Err(format!(
                    "RPC response did not reflect fixture preview at {expected}x: {status}"
                ));
            }
            measure_current_rate(expected)?;
        }
        println!("Ordinary speed RPC commands drove four already-installed Wine timer imports");
        exercise_rpc_hold_expiry(directory)?;
        Ok(())
    }

    /// Test-only rendezvous with the Linux-native speed CLI. The host runs
    /// Intimatr's actual Windows RPC client under the same disposable Wine
    /// prefix and asks this already-hooked executable to measure each rate.
    /// This is disabled unless the isolated CI harness opts in explicitly.
    fn exercise_external_linux_speed_bridge(directory: &Path) -> Result<(), String> {
        if std::env::var_os("INTIMATR_EXTERNAL_SPEED_BRIDGE").is_none() {
            return Ok(());
        }
        let ready = directory.join("bridge-ready.flag");
        std::fs::write(&ready, b"four imported timer hooks active")
            .map_err(|error| format!("cannot signal external bridge readiness: {error}"))?;
        for (stage, rate) in [(1, 2.0), (2, 0.5), (3, 1.0)] {
            let trigger = directory.join(format!("bridge-step-{stage}.flag"));
            let mut received = false;
            for _ in 0..600 {
                if trigger.is_file() {
                    received = true;
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if !received {
                return Err(format!(
                    "Linux speed manager did not signal stage {stage} within 30 seconds"
                ));
            }
            measure_current_rate(rate)?;
            let acknowledgement = directory.join(format!("bridge-ack-{stage}.flag"));
            std::fs::write(acknowledgement, format!("measured {rate}x"))
                .map_err(|error| format!("cannot acknowledge stage {stage}: {error}"))?;
        }
        println!("Linux Bottles speed CLI drove four hooked APIs through external Wine RPC");
        Ok(())
    }

    /// A read-only RPC status query must not restart a temporary rate.
    /// The IAT clock itself must expire the lease when callbacks are read.
    fn exercise_rpc_hold_expiry(directory: &Path) -> Result<(), String> {
        fn call(
            client: &Path,
            request: &[&str],
        ) -> Result<serde_json::Value, String> {
            let output = std::process::Command::new(client)
                .args(request)
                .output()
                .map_err(|error| format!("cannot start timed speed client: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "timed speed command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid timed speed JSON: {error}"))
        }

        let client = directory.join("intimatr-speed.exe");
        let hold = call(
            &client,
            &["--tcp", "127.0.0.1:31379", "hold", "2.0", "1800"],
        )?;
        if hold["preview_multiplier"] != 2.0 {
            return Err(format!("timed hold did not activate: {hold}"));
        }
        let read_only = call(&client, &["--tcp", "127.0.0.1:31379", "status"])?;
        if read_only["preview_multiplier"] != 2.0 {
            return Err(format!("status unexpectedly canceled active hold: {read_only}"));
        }
        measure_current_rate(2.0)?;
        thread::sleep(Duration::from_millis(1900));
        measure_current_rate(1.0)?;
        let expired = call(&client, &["--tcp", "127.0.0.1:31379", "status"])?;
        if expired["preview_multiplier"] != 1.0 {
            return Err(format!("held preview did not expire: {expired}"));
        }

        let held = call(
            &client,
            &["--tcp", "127.0.0.1:31379", "hold", "0.5", "2000"],
        )?;
        if held["preview_multiplier"] != 0.5 {
            return Err(format!("second hold not activated: {held}"));
        }
        let released = call(&client, &["--tcp", "127.0.0.1:31379", "release"])?;
        if released["preview_multiplier"] != 1.0 {
            return Err(format!("explicit hold release not applied: {released}"));
        }
        measure_current_rate(1.0)?;
        println!("Wine fixture timed hold expired without RPC polling and explicit release restored 1x");
        Ok(())
    }

    pub fn run() -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let directory = executable.parent().unwrap_or_else(|| Path::new("."));
        let dll = directory.join("intimatr.dll");
        if !dll.is_file() {
            return Err(format!(
                "test DLL missing beside executable: {}",
                dll.display()
            ));
        }

        let _preview_profile = TemporaryPreviewProfile::create()?;

        let wide: Vec<u16> = dll
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "LoadLibraryW failed for {}: {}",
                dll.display(),
                std::io::Error::last_os_error()
            ));
        }
        let module = LoadedModule(handle);

        let state_raw = unsafe { resolve(&module, c"intimatr_lifecycle_state")? };
        let shutdown_raw = unsafe { resolve(&module, c"intimatr_request_shutdown")? };
        let state: StateFn = unsafe { std::mem::transmute(state_raw) };
        let shutdown: ShutdownFn = unsafe { std::mem::transmute(shutdown_raw) };

        let mut running = false;
        for _ in 0..200 {
            let current = unsafe { state() };
            if current == 3 {
                running = true;
                break;
            }
            if current == 6 {
                return Err("Intimatr reported a failed bootstrap under Wine".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        if !running {
            return Err(format!(
                "Intimatr did not reach Running; lifecycle state={}",
                unsafe { state() }
            ));
        }
        println!("Intimatr DLL loaded in Wine process and reached Running");
        exercise_speed_cli(directory)?;

        // A constrained offline fixture is the only process allowed to enable
        // this experimental import hook. The probe verifies actual API time,
        // not just the separately tested pure virtual-clock arithmetic.
        let hook_enable: BoolFn =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_enable")?) };
        let hook_disable: BoolFn =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_disable")?) };
        let hook_count: unsafe extern "system" fn() -> u32 =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_hook_count")?) };
        let set_rate: RateFn =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_rate")?) };
        let fail_after: unsafe extern "system" fn(u32) -> i32 =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_fail_after")?) };
        let shared_rate: RateFn =
            unsafe { std::mem::transmute(resolve(&module, c"intimatr_timer_probe_shared_rate")?) };
        if unsafe { shared_rate(2.0) } != 0 {
            return Err("shared command activated a timer without installed hooks".into());
        }
        for stage in [0, 1, 2, 3] {
            if unsafe { fail_after(stage) } != 0 || unsafe { hook_count() } != 0 {
                return Err(format!(
                    "fault-injected rollback failed after {stage} imports"
                ));
            }
        }
        println!("Wine timer import rollback verified after 0/1/2/3 attempted hooks");
        if unsafe { hook_enable() } == 0 {
            return Err("fixture's QueryPerformanceCounter import was not hooked".into());
        }
        let count = unsafe { hook_count() };
        if unsafe { hook_enable() } == 0 || unsafe { hook_count() } != count {
            return Err("repeat timer hook enable was not idempotent".into());
        }
        if unsafe { set_rate(1.25) } != 0 {
            return Err("unsupported speed preset was unexpectedly accepted".into());
        }
        println!("Wine timer hooks installed on {count} import slots");
        if count < 4 {
            return Err(format!(
                "only {count} timer imports hooked; expected at least four"
            ));
        }
        exercise_live_hook_through_rpc(directory)?;
        exercise_external_linux_speed_bridge(directory)?;
        measure_rate(0.5, set_rate)?;
        measure_rate(2.0, set_rate)?;
        measure_rate(1.0, set_rate)?;
        // Unlike the direct test export, this route validates the preset
        // through Intimatr's shared RPC/UI command state first. The existing
        // real Wine IAT timing measurements then confirm the hook follows it.
        if unsafe { shared_rate(1.25) } != 0 {
            return Err("unsupported shared speed preset unexpectedly accepted".into());
        }
        measure_rate(0.5, shared_rate)?;
        measure_rate(2.0, shared_rate)?;
        measure_rate(1.0, shared_rate)?;
        println!("Shared speed commands drove four real Wine timer imports");
        stress_concurrent_timers(set_rate)?;
        if unsafe { hook_disable() } == 0 || unsafe { hook_count() } != 0 {
            return Err("experimental timer import restoration failed".into());
        }
        if unsafe { hook_disable() } == 0 {
            return Err("repeat timer hook disable was not idempotent".into());
        }
        if unsafe { shared_rate(2.0) } != 0 {
            return Err("shared command reactivated restored timer hooks".into());
        }
        println!("Wine timer hooks restored all original imports");

        // Reinstall the same fixture-only hooks, then ask the DLL to shut
        // down *without* calling the direct disable export. A safe shutdown
        // must restore every IAT pointer before reporting Stopped.
        if unsafe { hook_enable() } == 0 || unsafe { hook_count() } < 4 {
            return Err("cannot reinstall fixture timer imports for shutdown test".into());
        }
        if unsafe { shutdown() } == 0 {
            return Err("Intimatr exported shutdown returned failure".into());
        }
        if unsafe { hook_count() } != 0 {
            return Err(
                "DLL reported Stopped with experimental timer hooks still installed".into(),
            );
        }
        println!("DLL shutdown restored still-installed timer hooks");
        if unsafe { state() } != 5 {
            return Err("Intimatr did not reach Stopped after shutdown".into());
        }
        println!("Intimatr stopped cleanly before unload");
        Ok(())
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_probe::run() {
        eprintln!("Wine DLL smoke test failed: {error}");
        std::process::exit(1);
    }
}
