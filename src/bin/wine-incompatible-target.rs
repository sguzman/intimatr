//! Disposable offline Wine target intentionally lacking a static WinMM
//! timeGetTime import. A deliberate negative test of read-only compatibility
//! checks, not a game or a target for timing changes.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-incompatible-target is a Windows PE fixture run under Wine");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("wine-incompatible-target: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use std::{ffi::c_void, path::Path, thread, time::Duration};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
    }

    pub fn run() -> Result<(), String> {
        // Ensure winmm exists for the separate runtime import-address scan,
        // without importing its timeGetTime function in this executable.
        let winmm = "winmm.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        if unsafe { LoadLibraryW(winmm.as_ptr()) }.is_null() {
            return Err("cannot load WinMM for negative compatibility fixture".into());
        }
        let current = std::env::current_exe().map_err(|error| error.to_string())?;
        let directory = current.parent().unwrap_or_else(|| Path::new("."));
        std::fs::write(
            directory.join("incompatible-target.pid"),
            std::process::id().to_string(),
        )
        .map_err(|error| format!("cannot publish incompatible target PID: {error}"))?;
        let release = directory.join("incompatible-release.flag");
        for _ in 0..2400 {
            if release.is_file() {
                println!("Incompatible Wine target released without any timer hooking");
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err("negative compatibility target was not released".into())
    }
}
