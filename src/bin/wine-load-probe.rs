//! Small unprotected Windows process used to smoke-test Wine DLL loading.
//! It does not inject into any other process, scan memory, or alter game files.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-load-probe is a Windows executable built on a Linux host");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows_probe {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, thread, time::Duration};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(path: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    struct LoadedModule(*mut c_void);

    impl Drop for LoadedModule {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }

    type StateFn = unsafe extern "system" fn() -> u32;
    type ShutdownFn = unsafe extern "system" fn() -> i32;

    unsafe fn resolve(module: &LoadedModule, name: &'static [u8]) -> Result<*const c_void, String> {
        let address = unsafe { GetProcAddress(module.0, name.as_ptr()) };
        if address.is_null() {
            return Err(format!(
                "missing exported function {:?}",
                String::from_utf8_lossy(name)
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

        let state_raw = unsafe { resolve(&module, b"intimatr_lifecycle_state\0")? };
        let shutdown_raw = unsafe { resolve(&module, b"intimatr_request_shutdown\0")? };
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
