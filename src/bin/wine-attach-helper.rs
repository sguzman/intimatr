//! Explicit Windows PE helper launched from Linux under the SAME Wine prefix.
//!
//! This is a narrow feasibility test, not a general game injector: it accepts
//! only a PID for our own "wine-attach-target.exe" and loads "intimatr.dll"
//! from its own executable directory. No stealth, anti-cheat bypass, or
//! unrelated-process injection.

#[cfg(not(windows))]
fn main() {
    eprintln!("wine-attach-helper must be built for Windows and launched under Wine");
    std::process::exit(2);
}

#[cfg(windows)]
mod helper {
    use std::{
        ffi::c_void,
        fs,
        mem::{size_of, zeroed},
        os::windows::ffi::OsStrExt,
        path::Path,
        ptr,
    };

    type Handle = *mut c_void;
    const ACCESS: u32 = 0x0002 | 0x0008 | 0x0020 | 0x0400 | 0x1000;
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 0x04;
    const SNAP_MODULE: u32 = 0x0000_0008;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x80;
    const WAIT_TIMEOUT: u32 = 258;
    const ERROR_NO_MORE_FILES: u32 = 18;

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
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
        fn ReleaseMutex(mutex: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
        fn QueryFullProcessImageNameW(
            handle: Handle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn GetModuleHandleW(name: *const u16) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
        fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
        fn VirtualAllocEx(
            handle: Handle,
            address: *mut c_void,
            size: usize,
            flags: u32,
            protect: u32,
        ) -> *mut c_void;
        fn VirtualFreeEx(handle: Handle, address: *mut c_void, size: usize, flags: u32) -> i32;
        fn WriteProcessMemory(
            handle: Handle,
            address: *mut c_void,
            buffer: *const c_void,
            size: usize,
            wrote: *mut usize,
        ) -> i32;
        fn CreateRemoteThread(
            handle: Handle,
            attributes: *const c_void,
            stack_size: usize,
            start: Option<unsafe extern "system" fn(*mut c_void) -> u32>,
            argument: *mut c_void,
            flags: u32,
            thread_id: *mut u32,
        ) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    }

    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Serializes fixture attach attempts across Wine helper processes in
    /// the same Windows session. Not a cross-prefix or real-game lock.
    struct AttachMutex(OwnedHandle);

    impl Drop for AttachMutex {
        fn drop(&mut self) {
            unsafe { ReleaseMutex(self.0.0) };
        }
    }

    fn acquire_attach_mutex(pid: u32) -> Result<AttachMutex, String> {
        let name = format!("Local\\IntimatrFixtureAttach-{pid}");
        let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mutex = own(
            unsafe { CreateMutexW(ptr::null(), 0, wide_name.as_ptr()) },
            "cannot create per-target attachment mutex",
        )?;
        match unsafe { WaitForSingleObject(mutex.0, 15_000) } {
            WAIT_OBJECT_0 => Ok(AttachMutex(mutex)),
            WAIT_ABANDONED => {
                // An abandoned owner could have left a remote loader running.
                // We now own the mutex, but must not proceed with loading.
                unsafe { ReleaseMutex(mutex.0) };
                Err("refusing attach: previous helper abandoned its lock".into())
            }
            WAIT_TIMEOUT => Err("per-target attachment mutex timed out".into()),
            _ => Err(format!(
                "could not acquire attachment mutex: {}",
                std::io::Error::last_os_error()
            )),
        }
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn own(handle: Handle, detail: &str) -> Result<OwnedHandle, String> {
        if handle.is_null() || handle as usize == usize::MAX {
            return Err(format!("{detail}: {}", std::io::Error::last_os_error()));
        }
        Ok(OwnedHandle(handle))
    }

    /// Compare the complete target executable identity, not its filename.
    /// Only the known disposable fixture physically beside THIS helper may
    /// receive the test DLL. A same-named executable elsewhere is not enough.
    fn same_owned_fixture_image(
        observed_image: &Path,
        helper_image: &Path,
    ) -> Result<bool, String> {
        let helper_dir = helper_image
            .parent()
            .ok_or("helper has no executable directory")?;
        let expected_image = helper_dir.join("wine-attach-target.exe");
        let expected = fs::canonicalize(expected_image)
            .map_err(|error| format!("cannot resolve owned test fixture: {error}"))?;
        let observed = fs::canonicalize(observed_image)
            .map_err(|error| format!("cannot resolve observed target executable: {error}"))?;
        Ok(expected
            .to_string_lossy()
            .eq_ignore_ascii_case(&observed.to_string_lossy()))
    }

    fn target_is_fixture(handle: Handle, helper_image: &Path) -> Result<bool, String> {
        let mut path = [0_u16; 2048];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return Err(format!(
                "cannot identify process: {}",
                std::io::Error::last_os_error()
            ));
        }
        let image = String::from_utf16_lossy(&path[..length as usize]);
        same_owned_fixture_image(Path::new(&image), helper_image)
    }

    /// Module32NextW returns zero both at the normal end of a snapshot
    /// and on errors. Only ERROR_NO_MORE_FILES proves an exhaustive scan.
    fn complete_module_scan(error: u32) -> Result<(), String> {
        if error == ERROR_NO_MORE_FILES {
            Ok(())
        } else {
            Err(format!(
                "target module enumeration incomplete (Windows error {error})"
            ))
        }
    }

    /// Fail closed when the target module list cannot be inspected.
    /// In particular, never issue a second remote LoadLibrary for a target
    /// already carrying Intimatr. The caller holds the fixture-only named
    /// mutex across checking, loading, and post-load module verification.
    fn target_has_intimatr(pid: u32) -> Result<bool, String> {
        let snapshot = own(
            unsafe { CreateToolhelp32Snapshot(SNAP_MODULE, pid) },
            "cannot inspect target DLLs",
        )?;
        let mut entry: ModuleEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ModuleEntry32W>() as u32;
        if unsafe { Module32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err("cannot inspect the first target module".into());
        }
        loop {
            let end = entry
                .name
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(entry.name.len());
            let name = String::from_utf16_lossy(&entry.name[..end]);
            if name.eq_ignore_ascii_case("intimatr.dll") {
                return Ok(true);
            }
            if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
                // Capture GetLastError immediately after Module32NextW.
                // Any other error is an incomplete scan, not proof that
                // Intimatr is absent.
                complete_module_scan(unsafe { GetLastError() })?;
                break;
            }
        }
        Ok(false)
    }

    fn local_image_size(base: usize) -> Option<usize> {
        let bytes = base as *const u8;
        let dos = unsafe { ptr::read_unaligned(bytes.cast::<u16>()) };
        if dos != 0x5a4d {
            return None;
        }
        let pe = unsafe { ptr::read_unaligned(bytes.add(0x3c).cast::<u32>()) } as usize;
        if pe > 2048 {
            return None;
        }
        let signature = unsafe { ptr::read_unaligned(bytes.add(pe).cast::<u32>()) };
        if signature != 0x0000_4550 {
            return None;
        }
        let optional = pe + 24;
        let magic = unsafe { ptr::read_unaligned(bytes.add(optional).cast::<u16>()) };
        if magic != 0x20b {
            return None;
        }
        let size = unsafe { ptr::read_unaligned(bytes.add(optional + 56).cast::<u32>()) } as usize;
        (4096..=128 * 1024 * 1024).contains(&size).then_some(size)
    }

    fn resolve_remote_loader(pid: u32) -> Result<usize, String> {
        let kernel =
            unsafe { GetModuleHandleW(wide(std::ffi::OsStr::new("kernel32.dll")).as_ptr()) };
        if kernel.is_null() {
            return Err("helper cannot find kernel32".into());
        }
        let address = unsafe { GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()) } as usize;
        if address == 0 {
            return Err("helper cannot resolve LoadLibraryW".into());
        }

        // A forwarded Windows export may live in kernelbase rather than in
        // kernel32. Identify the actual local PE image, then find the matching
        // module inside the target via the Windows module-snapshot API.
        let candidates = ["kernel32.dll", "kernelbase.dll", "ntdll.dll"];
        let mut selected = None;
        for module_name in candidates {
            let loaded =
                unsafe { GetModuleHandleW(wide(std::ffi::OsStr::new(module_name)).as_ptr()) }
                    as usize;
            if loaded == 0 {
                continue;
            }
            if let Some(image_size) = local_image_size(loaded)
                && address >= loaded
                && address < loaded.saturating_add(image_size)
            {
                selected = Some((module_name, address - loaded));
                break;
            }
        }
        let (module_name, rva) = selected.ok_or_else(|| {
            "LoadLibraryW address does not belong to a known system module".to_owned()
        })?;

        let snapshot = own(
            unsafe { CreateToolhelp32Snapshot(SNAP_MODULE, pid) },
            "cannot enumerate target modules",
        )?;
        let mut entry: ModuleEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ModuleEntry32W>() as u32;
        if unsafe { Module32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err("target module list is empty".into());
        }
        loop {
            let end = entry
                .name
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.name.len());
            let current = String::from_utf16_lossy(&entry.name[..end]);
            if current.eq_ignore_ascii_case(module_name) {
                if rva >= entry.size_bytes as usize {
                    return Err("remote system DLL is incompatible with the helper".into());
                }
                return Ok((entry.base as usize).saturating_add(rva));
            }
            if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
        Err(format!("target has no loaded {module_name}"))
    }

    pub fn run() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let pid: u32 = args
            .next()
            .ok_or("expected a single PID")?
            .parse()
            .map_err(|_| "invalid PID")?;
        if pid == 0 || args.next().is_some() {
            return Err("usage: wine-attach-helper <test-target-PID>".into());
        }
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let dll = executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("intimatr.dll");
        if !dll.is_file() {
            return Err(format!("cannot find test DLL: {}", dll.display()));
        }
        let process = own(
            unsafe { OpenProcess(ACCESS, 0, pid) },
            "cannot open test target",
        )?;
        if !target_is_fixture(process.0, &executable)? {
            return Err("refusing to attach: target is not the helper's owned Wine fixture".into());
        }
        let _attachment_guard = acquire_attach_mutex(pid)?;
        if target_has_intimatr(pid)? {
            return Err("refusing duplicate attach: Intimatr is already loaded".into());
        }
        let loader = resolve_remote_loader(pid)?;
        let path = wide(dll.as_os_str());
        let size = path.len() * size_of::<u16>();
        let remote = unsafe {
            VirtualAllocEx(
                process.0,
                ptr::null_mut(),
                size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if remote.is_null() {
            return Err(format!(
                "cannot allocate remote path: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut wrote = 0_usize;
        if unsafe { WriteProcessMemory(process.0, remote, path.as_ptr().cast(), size, &mut wrote) }
            == 0
            || wrote != size
        {
            unsafe { VirtualFreeEx(process.0, remote, 0, MEM_RELEASE) };
            return Err("cannot write test DLL path into target".into());
        }
        // Resolve to an actual module RVA in the TARGET rather than assuming
        // the helper's kernel32/kernelbase mapping has the same absolute VA.
        let start: unsafe extern "system" fn(*mut c_void) -> u32 =
            unsafe { std::mem::transmute(loader) };
        let thread = unsafe {
            CreateRemoteThread(
                process.0,
                ptr::null(),
                0,
                Some(start),
                remote,
                0,
                ptr::null_mut(),
            )
        };
        if thread.is_null() {
            unsafe { VirtualFreeEx(process.0, remote, 0, MEM_RELEASE) };
            return Err(format!(
                "cannot create test loading thread: {}",
                std::io::Error::last_os_error()
            ));
        }
        let thread = OwnedHandle(thread);
        match unsafe { WaitForSingleObject(thread.0, 10_000) } {
            WAIT_OBJECT_0 => {
                unsafe { VirtualFreeEx(process.0, remote, 0, MEM_RELEASE) };
                // A signaled loader thread does not by itself prove that the
                // DLL was loaded. Require a target-side module observation.
                if !target_has_intimatr(pid)? {
                    return Err("remote loader exited without Intimatr in the target".into());
                }
                println!("Wine helper confirmed Intimatr module in existing test PID {pid}");
                Ok(())
            }
            WAIT_TIMEOUT => {
                // The remote thread may still be reading this argument.
                // Deliberately leak it rather than use-after-free.
                Err("remote test loader timed out; retaining argument buffer".into())
            }
            _ => Err("remote test loader wait failed; retaining argument buffer".into()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{ERROR_NO_MORE_FILES, complete_module_scan, same_owned_fixture_image};
        use std::{
            fs,
            sync::atomic::{AtomicUsize, Ordering},
        };

        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        #[test]
        fn incomplete_module_enumeration_is_not_proof_of_absence() {
            assert!(complete_module_scan(ERROR_NO_MORE_FILES).is_ok());
            for code in [0, 5, 6, 24, 299, 998] {
                assert!(complete_module_scan(code).is_err());
            }
        }

        #[test]
        fn same_basename_outside_owned_fixture_directory_is_rejected() {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "intimatr-owned-fixture-{}-{id}",
                std::process::id()
            ));
            let owned_dir = root.join("owned");
            let other_dir = root.join("unrelated");
            fs::create_dir_all(&owned_dir).unwrap();
            fs::create_dir_all(&other_dir).unwrap();
            let fixture = owned_dir.join("wine-attach-target.exe");
            let decoy = other_dir.join("wine-attach-target.exe");
            fs::write(&fixture, b"owned fixture").unwrap();
            fs::write(&decoy, b"unrelated same-name executable").unwrap();
            let helper = owned_dir.join("wine-attach-helper.exe");

            assert!(same_owned_fixture_image(&fixture, &helper).unwrap());
            assert!(!same_owned_fixture_image(&decoy, &helper).unwrap());
            assert!(same_owned_fixture_image(&other_dir.join("missing.exe"), &helper).is_err());

            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = helper::run() {
        eprintln!("Wine attach helper: {error}");
        std::process::exit(1);
    }
}
