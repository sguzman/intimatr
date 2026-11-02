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
        os::windows::ffi::OsStrExt,
        path::Path,
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

    struct LoadedModule(*mut c_void);

    impl Drop for LoadedModule {
        fn drop(&mut self) {
            // Errors while a hook is installed must never unload a DLL whose
            // callbacks could still be referenced by this executable.
            let disable = unsafe {
                GetProcAddress(self.0, c"intimatr_timer_probe_disable".as_ptr().cast())
            };
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

    fn measure_millisecond_timers(rate: f64) -> Result<(), String> {
        let virtual64_before = unsafe { GetTickCount64() };
        let real64_before = unsafe { original_tick64()? };
        let virtual32_before = unsafe { GetTickCount() };
        let real32_before = unsafe { original_tick32()? };
        thread::sleep(Duration::from_millis(160));
        let virtual64_after = unsafe { GetTickCount64() };
        let real64_after = unsafe { original_tick64()? };
        let virtual32_after = unsafe { GetTickCount() };
        let real32_after = unsafe { original_tick32()? };
        let real64_delta = real64_after.saturating_sub(real64_before);
        let real32_delta = real32_after.wrapping_sub(real32_before);
        if real64_delta == 0 || real32_delta == 0 {
            return Err("unmodified Windows millisecond timers did not advance".to_owned());
        }
        let ratio64 = virtual64_after.saturating_sub(virtual64_before) as f64 / real64_delta as f64;
        let ratio32 = virtual32_after.wrapping_sub(virtual32_before) as f64 / real32_delta as f64;
        println!(
            "Wine ms timer hooks: requested={rate:.2}x tick64={ratio64:.3}x tick32={ratio32:.3}x"
        );
        if (ratio64 - rate).abs() > 0.30 || (ratio32 - rate).abs() > 0.30 {
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
        for stage in [0, 1, 2] {
            if unsafe { fail_after(stage) } != 0 || unsafe { hook_count() } != 0 {
                return Err(format!("fault-injected rollback failed after {stage} imports"));
            }
        }
        println!("Wine timer import rollback verified after 0/1/2 attempted hooks");
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
        if count < 3 {
            return Err(format!(
                "only {count} timer imports hooked; expected at least three"
            ));
        }
        measure_rate(0.5, set_rate)?;
        measure_rate(2.0, set_rate)?;
        measure_rate(1.0, set_rate)?;
        if unsafe { hook_disable() } == 0 || unsafe { hook_count() } != 0 {
            return Err("experimental timer import restoration failed".into());
        }
        if unsafe { hook_disable() } == 0 {
            return Err("repeat timer hook disable was not idempotent".into());
        }
        println!("Wine timer hooks restored all original imports");

        if unsafe { shutdown() } == 0 {
            return Err("Intimatr exported shutdown returned failure".into());
        }
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
