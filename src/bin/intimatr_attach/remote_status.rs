//! Confirm Intimatr's target-side bootstrap state after a remote LoadLibrary.
//! Loading a PE module is not proof that its asynchronous bootstrap succeeded.
//! This uses a zero-argument, read-only Intimatr export with the exact Windows
//! remote thread ABI; no timer hook, patch, or unrelated module is invoked.

use std::{
    ffi::c_void,
    fs,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr, thread,
    time::Duration,
};

type Handle = *mut c_void;
const SNAP_MODULE: u32 = 0x0000_0008;
const WAIT_OBJECT_0: u32 = 0;
const ERROR_NO_MORE_FILES: u32 = 18;
const DONT_RESOLVE_DLL_REFERENCES: u32 = 0x0000_0001;
const STATUS_EXPORT: &std::ffi::CStr = c"intimatr_remote_lifecycle_query";
const SHUTDOWN_EXPORT: &std::ffi::CStr = c"intimatr_remote_shutdown";

#[repr(C)]
#[allow(dead_code)]
struct ModuleEntry32W {
    size: u32,
    module_id: u32,
    process_id: u32,
    global_count: u32,
    process_count: u32,
    base: *mut u8,
    size_bytes: u32,
    module: Handle,
    name: [u16; 256],
    path: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn LoadLibraryExW(path: *const u16, file: Handle, flags: u32) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    fn FreeLibrary(module: Handle) -> i32;
    fn CreateRemoteThread(
        process: Handle,
        attributes: *const c_void,
        stack_size: usize,
        start: Option<unsafe extern "system" fn(*mut c_void) -> u32>,
        argument: *mut c_void,
        flags: u32,
        thread_id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeThread(thread: Handle, code: *mut u32) -> i32;
}

struct Snapshot(Handle);
impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
struct LocalModule(Handle);
impl Drop for LocalModule {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}
struct RemoteThread(Handle);
impl Drop for RemoteThread {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn canonical_match(observed: &Path, expected: &Path) -> Result<bool, String> {
    let observed = fs::canonicalize(observed)
        .map_err(|error| format!("cannot canonicalize target Intimatr module: {error}"))?;
    let expected = fs::canonicalize(expected)
        .map_err(|error| format!("cannot canonicalize selected Intimatr DLL: {error}"))?;
    Ok(observed
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.to_string_lossy()))
}

/// Return the address of our exact module, never a basename-only collision.
fn target_base(pid: u32, dll: &Path) -> Result<(usize, usize), String> {
    let handle = unsafe { CreateToolhelp32Snapshot(SNAP_MODULE, pid) };
    if handle.is_null() || handle as usize == usize::MAX {
        return Err("cannot enumerate target modules for Intimatr lifecycle query".into());
    }
    let snapshot = Snapshot(handle);
    let mut entry: ModuleEntry32W = unsafe { zeroed() };
    entry.size = size_of::<ModuleEntry32W>() as u32;
    if unsafe { Module32FirstW(snapshot.0, &mut entry) } == 0 {
        return Err("cannot read target modules for Intimatr lifecycle query".into());
    }
    loop {
        let name_end = entry
            .name
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.name.len());
        let name = String::from_utf16(&entry.name[..name_end])
            .map_err(|_| "invalid UTF-16 in target module name")?;
        if name.eq_ignore_ascii_case("intimatr.dll") {
            let end = entry
                .path
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.path.len());
            if end == 0 {
                return Err("target Intimatr module path is missing".into());
            }
            let found = String::from_utf16(&entry.path[..end])
                .map_err(|_| "invalid UTF-16 in target Intimatr module path")?;
            if !canonical_match(Path::new(&found), dll)? {
                return Err("target has another Intimatr DLL; lifecycle query refused".into());
            }
            if entry.base.is_null() || entry.size_bytes == 0 {
                return Err("target Intimatr DLL has no valid image range".into());
            }
            return Ok((entry.base as usize, entry.size_bytes as usize));
        }
        if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
            if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
                return Err("target Intimatr module enumeration was incomplete".into());
            }
            break;
        }
    }
    Err("target Intimatr DLL disappeared before lifecycle query".into())
}

/// Bounds-check the mapped image, not an arbitrary pointer from the caller.
fn local_image_size(base: usize) -> Option<usize> {
    let bytes = base as *const u8;
    if unsafe { ptr::read_unaligned(bytes.cast::<u16>()) } != 0x5a4d {
        return None;
    }
    let pe = unsafe { ptr::read_unaligned(bytes.add(0x3c).cast::<u32>()) } as usize;
    if pe > 2048 || unsafe { ptr::read_unaligned(bytes.add(pe).cast::<u32>()) } != 0x0000_4550 {
        return None;
    }
    let optional = pe + 24;
    if unsafe { ptr::read_unaligned(bytes.add(optional).cast::<u16>()) } != 0x20b {
        return None;
    }
    let size = unsafe { ptr::read_unaligned(bytes.add(optional + 56).cast::<u32>()) } as usize;
    (4096..=128 * 1024 * 1024).contains(&size).then_some(size)
}

/// Map the *same DLL file* without running DllMain, then take a bounded RVA
/// for our exact, non-forwarded export; ASLR is handled by the remote module
/// base. No local bootstrap and no second target-side LoadLibrary is issued.
fn export_rva(dll: &Path, export: &std::ffi::CStr) -> Result<usize, String> {
    let wide: Vec<u16> = dll
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mapped =
        unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), DONT_RESOLVE_DLL_REFERENCES) };
    if mapped.is_null() {
        return Err("cannot inspect owned Intimatr lifecycle export without executing it".into());
    }
    let module = LocalModule(mapped);
    let base = module.0 as usize;
    let size = local_image_size(base).ok_or("invalid local Intimatr PE image")?;
    let address =
        unsafe { GetProcAddress(module.0, export.as_ptr().cast()) } as usize;
    if address < base || address >= base.saturating_add(size) {
        return Err("owned Intimatr lifecycle export is missing or forwarded".into());
    }
    Ok(address - base)
}

fn call_export(process: Handle, remote: usize, timeout_ms: u32) -> Result<u32, String> {
    let entry: unsafe extern "system" fn(*mut c_void) -> u32 =
        unsafe { std::mem::transmute(remote) };
    let thread = unsafe {
        CreateRemoteThread(
            process,
            ptr::null(),
            0,
            Some(entry),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        )
    };
    if thread.is_null() {
        return Err("cannot run Intimatr's read-only lifecycle query in selected process".into());
    }
    let thread = RemoteThread(thread);
    if unsafe { WaitForSingleObject(thread.0, timeout_ms) } != WAIT_OBJECT_0 {
        return Err(
            "Intimatr remote lifecycle query timed out or failed; module state unknown".into(),
        );
    }
    let mut status = 0;
    if unsafe { GetExitCodeThread(thread.0, &mut status) } == 0 {
        return Err("cannot read Intimatr's remote lifecycle status".into());
    }
    Ok(status)
}

fn classify(code: u32) -> Result<bool, String> {
    match code {
        3 => Ok(true),      // Running
        0..=2 => Ok(false), // Detached/Attached/Bootstrapping
        4..=5 => Err("Intimatr stopped during target bootstrap".into()),
        6 => Err("Intimatr reported failed target bootstrap".into()),
        _ => Err(format!("unknown Intimatr lifecycle code {code}")),
    }
}

/// Return only after the selected DLL reports Running from the actual target.
/// Absence of this proof means loaded but not runtime-verified; caller must
/// never advertise speed functionality or automatic cleanup.
pub fn wait_until_running(process: Handle, pid: u32, dll: &Path) -> Result<(), String> {
    let (base, remote_image_size) = target_base(pid, dll)?;
    let rva = export_rva(dll, STATUS_EXPORT)?;
    if rva >= remote_image_size {
        return Err("target Intimatr module does not contain the lifecycle query export".into());
    }
    let entry = base
        .checked_add(rva)
        .ok_or("remote Intimatr lifecycle entry overflow")?;
    for _ in 0..40 {
        if classify(call_export(process, entry, 2_000)?)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("Intimatr DLL loaded but runtime never reached Running; do not retry attach".into())
}

/// Explicitly call only the selected Intimatr DLL's own shutdown export.
/// Leave the DLL mapped and independently require its Stopped state.
pub fn request_shutdown(process: Handle, pid: u32, dll: &Path) -> Result<(), String> {
    let (base, image_size) = target_base(pid, dll)?;
    let shutdown_rva = export_rva(dll, SHUTDOWN_EXPORT)?;
    let status_rva = export_rva(dll, STATUS_EXPORT)?;
    if shutdown_rva >= image_size || status_rva >= image_size {
        return Err("target Intimatr DLL lacks bounded lifecycle exports".into());
    }
    let shutdown = base.checked_add(shutdown_rva).ok_or("remote Intimatr shutdown address overflow")?;
    let status = base.checked_add(status_rva).ok_or("remote Intimatr status address overflow")?;
    if call_export(process, shutdown, 15_000)? != 1 {
        return Err("target Intimatr shutdown failed; DLL remains loaded".into());
    }
    if call_export(process, status, 2_000)? != 5 {
        return Err("target Intimatr did not confirm Stopped after shutdown".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_status_is_not_module_presence() {
        for code in 0..=2 {
            assert!(!classify(code).unwrap());
        }
        assert!(classify(3).unwrap());
        for code in [4, 5, 6, 7, u32::MAX] {
            assert!(classify(code).is_err());
        }
    }
}
