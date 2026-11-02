//! Read-only Windows PID enumeration inside the selected Wine prefix.
//! Built as PE on Linux, launched with WINEPREFIX by intimatr-bottles.
//! No opening for injection, debugger access, or game modification.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-process-list is a Windows PE helper launched inside Wine");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows {
    use intimatr::pe_image::inspect_pe;
    use serde::Serialize;
    use std::{
        ffi::c_void,
        mem::{size_of, zeroed},
    };

    type Handle = *mut c_void;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[repr(C)]
    #[allow(dead_code)]
    struct ProcessEntry32W {
        size: u32,
        usage_count: u32,
        process_id: u32,
        default_heap: usize,
        module_id: u32,
        thread_count: u32,
        parent_process_id: u32,
        priority_class: i32,
        flags: u32,
        exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            buffer: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    struct OwnedHandle(Handle);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    #[derive(Debug, Serialize)]
    struct WindowsProcess {
        windows_pid: u32,
        reported_executable: String,
        full_image_path: Option<String>,
        image_query_verified: bool,
        pe_image_architecture: Option<&'static str>,
        pe_image_verified: bool,
        safe_to_attach: bool,
    }

    #[derive(Serialize)]
    struct WindowsProcessReport {
        operation: &'static str,
        processes: Vec<WindowsProcess>,
        attachment_attempted: bool,
    }

    fn utf16_z(values: &[u16]) -> String {
        let length = values
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(values.len());
        String::from_utf16_lossy(&values[..length])
    }

    fn image_path(pid: u32) -> Option<String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let owned = OwnedHandle(handle);
        let mut buffer = [0_u16; 2048];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(owned.0, 0, buffer.as_mut_ptr(), &mut length) } == 0
        {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }

    fn run() -> Result<(), String> {
        if std::env::args_os().len() != 1 {
            return Err("wine-process-list does not accept arguments".to_owned());
        }
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot.is_null() || snapshot as usize == usize::MAX {
            return Err(format!(
                "cannot enumerate Windows processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry: ProcessEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ProcessEntry32W>() as u32;
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err(format!(
                "Windows process snapshot is empty or unreadable: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut processes = Vec::new();
        loop {
            let pid = entry.process_id;
            if pid > 0 {
                let path = image_path(pid);
                // Parsing PE headers from this reported file path is
                // read-only evidence of FILE machine, not proof of process
                // machine, image ownership, or safe injection.
                let architecture = path
                    .as_deref()
                    .and_then(|path| inspect_pe(std::path::Path::new(path)).ok())
                    .map(|identity| identity.architecture);
                processes.push(WindowsProcess {
                    windows_pid: pid,
                    reported_executable: utf16_z(&entry.exe_file),
                    image_query_verified: path.is_some(),
                    full_image_path: path,
                    pe_image_architecture: architecture,
                    pe_image_verified: architecture.is_some(),
                    safe_to_attach: false,
                });
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
        processes.sort_by_key(|process| process.windows_pid);
        println!(
            "{}",
            serde_json::to_string_pretty(&WindowsProcessReport {
                operation: "read_only_wine_windows_processes",
                processes,
                attachment_attempted: false,
            })
            .map_err(|error| format!("cannot serialize Windows processes: {error}"))?
        );
        Ok(())
    }

    pub fn main() {
        if let Err(error) = run() {
            eprintln!("wine-process-list: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
fn main() {
    windows::main();
}
