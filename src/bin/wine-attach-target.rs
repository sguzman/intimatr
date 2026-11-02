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
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, thread, time::Duration};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(module: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }

    type StateFn = unsafe extern "system" fn() -> u32;
    type ShutdownFn = unsafe extern "system" fn() -> i32;

    pub fn run() -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let directory = executable.parent().unwrap_or_else(|| Path::new("."));
        let pid_file = directory.join("attach-target.pid");
        std::fs::write(&pid_file, std::process::id().to_string())
            .map_err(|error| format!("cannot write target PID: {error}"))?;
        println!("Offline Wine attach target awaiting DLL; pid={}", std::process::id());

        let wide: Vec<u16> = "intimatr.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        for _ in 0..800 {
            let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
            if !module.is_null() {
                let state = unsafe {
                    GetProcAddress(module, c"intimatr_lifecycle_state".as_ptr().cast())
                };
                let shutdown = unsafe {
                    GetProcAddress(module, c"intimatr_request_shutdown".as_ptr().cast())
                };
                if state.is_null() || shutdown.is_null() {
                    return Err("remote-loaded DLL is missing lifecycle exports".into());
                }
                let state: StateFn = unsafe { std::mem::transmute(state) };
                let stop: ShutdownFn = unsafe { std::mem::transmute(shutdown) };
                for _ in 0..200 {
                    match unsafe { state() } {
                        3 => {
                            // Allow the remote helper to finish its load thread
                            // before this deliberately controlled target exits.
                            thread::sleep(Duration::from_millis(250));
                            if unsafe { stop() } == 0 || unsafe { state() } != 5 {
                                return Err("attached DLL failed clean shutdown".into());
                            }
                            println!("Wine attach target observed Running then Stopped");
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
