#[cfg(windows)]
#[path = "remote_status.rs"]
mod remote_status;

#[cfg(windows)]
pub mod helper {
    use intimatr::{
        speed_policy::game_path_key, windows_process_identity::process_creation_time_100ns,
        windows_process_machine::running_process_architecture,
    };
    use std::{
        ffi::c_void,
        fs,
        mem::{size_of, zeroed},
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
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
        // Both fixture and selected-game helpers share this same lock.
        let name = format!("Local\\IntimatrAttach-{pid}");
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

    fn query_target_image(handle: Handle) -> Result<PathBuf, String> {
        let mut path = [0_u16; 2048];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return Err(format!(
                "cannot identify process: {}",
                std::io::Error::last_os_error()
            ));
        }
        let image = String::from_utf16(&path[..length as usize])
            .map_err(|_| "target process image is not valid UTF-16")?;
        Ok(PathBuf::from(image))
    }

    // Compiled into two deliberately separate binaries; each uses one mode.
    #[allow(dead_code)]
    enum AttachTarget {
        OwnedFixture,
        SelectedOfflineGame { path: PathBuf, created_100ns: u64 },
    }

    fn validate_target_image(
        handle: Handle,
        helper_image: &Path,
        target: &AttachTarget,
    ) -> Result<(), String> {
        let observed = query_target_image(handle)?;
        match target {
            AttachTarget::OwnedFixture => {
                if !same_owned_fixture_image(&observed, helper_image)? {
                    return Err(
                        "refusing to attach: target is not the helper's owned Wine fixture".into(),
                    );
                }
            }
            AttachTarget::SelectedOfflineGame { path: expected, .. } => {
                game_path_key(&expected.to_string_lossy())
                    .map_err(|error| format!("invalid offline game executable path: {error}"))?;
                let observed = fs::canonicalize(&observed)
                    .map_err(|error| format!("cannot resolve running game image: {error}"))?;
                let expected = fs::canonicalize(expected)
                    .map_err(|error| format!("cannot resolve selected game image: {error}"))?;
                if !observed
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.to_string_lossy())
                {
                    return Err("refusing attach: running game image differs from selection".into());
                }
                // These are never eligible offline games, even if a caller
                // supplies their exact full executable path.
                let name = observed
                    .file_name()
                    .map(|part| part.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if matches!(
                    name.as_str(),
                    "explorer.exe"
                        | "services.exe"
                        | "svchost.exe"
                        | "wine-attach-helper.exe"
                        | "wine-game-attach-helper.exe"
                        | "wine-process-list.exe"
                        | "rundll32.exe"
                ) {
                    return Err("refusing attach: selected Windows program is not a game".into());
                }
            }
        }
        Ok(())
    }

    /// Verify the exact Windows process generation on the held HANDLE.
    /// Matching PID/path by itself cannot rule out a replaced process.
    fn validate_target_generation(handle: Handle, target: &AttachTarget) -> Result<(), String> {
        if let AttachTarget::SelectedOfflineGame { created_100ns, .. } = target {
            let observed = unsafe { process_creation_time_100ns(handle) }
                .ok_or("refusing attach: cannot verify game process creation time")?;
            if observed != *created_100ns {
                return Err("refusing attach: game process creation-time identity mismatch".into());
            }
        }
        Ok(())
    }

    /// This is a conservative veto for common protection modules, *not*
    /// proof that an arbitrary game is unprotected. Never bypass protection.
    fn reject_observed_protection_modules(pid: u32) -> Result<(), String> {
        let snapshot = own(
            unsafe { CreateToolhelp32Snapshot(SNAP_MODULE, pid) },
            "cannot inspect protected game modules",
        )?;
        let mut entry: ModuleEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ModuleEntry32W>() as u32;
        if unsafe { Module32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err("cannot inspect game module list for protection".into());
        }
        loop {
            let end = entry
                .name
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(entry.name.len());
            let name = String::from_utf16(&entry.name[..end])
                .map_err(|_| "invalid UTF-16 in game module inventory")?
                .to_ascii_lowercase();
            if [
                "easyanticheat",
                "battleye",
                "xigncode",
                "nprotect",
                "anticheat",
                "faceit",
                "vgk",
                "eac_",
            ]
            .iter()
            .any(|part| name.contains(part))
            {
                return Err("refusing attach: game has a known protection module loaded".into());
            }
            if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
                complete_module_scan(unsafe { GetLastError() })?;
                break;
            }
        }
        Ok(())
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum IntimatrModulePresence {
        Absent,
        Expected,
        UnexpectedPath,
    }

    /// The module basename is sufficient to reject pre-existing collisions,
    /// but successful post-load verification must identify our exact DLL.
    fn classify_intimatr_module_path(
        observed_dll: &Path,
        expected_dll: &Path,
    ) -> Result<IntimatrModulePresence, String> {
        let observed = fs::canonicalize(observed_dll)
            .map_err(|error| format!("cannot resolve observed Intimatr DLL path: {error}"))?;
        let expected = fs::canonicalize(expected_dll)
            .map_err(|error| format!("cannot resolve owned Intimatr DLL path: {error}"))?;
        Ok(
            if observed
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy())
            {
                IntimatrModulePresence::Expected
            } else {
                IntimatrModulePresence::UnexpectedPath
            },
        )
    }

    /// Fail closed when the target module list cannot be inspected.
    /// In particular, never issue a second remote LoadLibrary for a target
    /// already carrying ANY same-named Intimatr DLL. The caller holds the
    /// fixture-only mutex across checking, loading, and post-load inspection.
    fn target_intimatr_module(
        pid: u32,
        expected_dll: &Path,
    ) -> Result<IntimatrModulePresence, String> {
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
                let end = entry
                    .path
                    .iter()
                    .position(|character| *character == 0)
                    .unwrap_or(entry.path.len());
                if end == 0 {
                    return Err("Intimatr module has no verifiable full image path".into());
                }
                let image = String::from_utf16(&entry.path[..end])
                    .map_err(|_| "Intimatr module has an invalid UTF-16 image path")?;
                return classify_intimatr_module_path(Path::new(&image), expected_dll);
            }
            if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
                // Capture GetLastError immediately after Module32NextW.
                // Any other error is an incomplete scan, not proof that
                // Intimatr is absent.
                complete_module_scan(unsafe { GetLastError() })?;
                break;
            }
        }
        Ok(IntimatrModulePresence::Absent)
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

    // This shared module is compiled by the game-only binary as well.
    #[allow(dead_code)]
    pub fn run_fixture() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let pid: u32 = args
            .next()
            .ok_or("expected a single PID")?
            .parse()
            .map_err(|_| "invalid PID")?;
        if pid == 0 || args.next().is_some() {
            return Err("usage: wine-attach-helper <test-target-PID>".into());
        }
        attach(pid, AttachTarget::OwnedFixture)
    }

    // This shared module is compiled by the fixture-only binary as well.
    #[allow(dead_code)]
    pub fn run_selected_game() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() != Some("--pid") {
            return Err("expected --pid WINDOWSPID".into());
        }
        let pid: u32 = args
            .next()
            .ok_or("missing Windows PID")?
            .parse()
            .map_err(|_| "invalid Windows PID")?;
        if pid == 0 || args.next().as_deref() != Some("--exe") {
            return Err("expected --exe EXACT_WINDOWS_GAME_PATH after nonzero --pid".into());
        }
        let expected = args.next().ok_or("missing selected game executable path")?;
        game_path_key(&expected)
            .map_err(|error| format!("invalid selected Windows game path: {error}"))?;
        if args.next().as_deref() != Some("--created-100ns") {
            return Err(
                "game attach requires --created-100ns from a fresh process snapshot".into(),
            );
        }
        let created_100ns = args
            .next()
            .ok_or("missing game process creation time")?
            .parse::<u64>()
            .map_err(|_| "invalid game process creation time")?;
        if created_100ns == 0 {
            return Err("game process creation time must be nonzero".into());
        }
        if args.next().as_deref() != Some("--offline-single-player") {
            return Err(
                "game action requires explicit --offline-single-player acknowledgment".into(),
            );
        }
        let (action, preset, milli_rate, step_action) = match args.next().as_deref() {
            None => ("attach", None, None, None),
            Some("--shutdown-only") if args.next().is_none() => ("stop", None, None, None),
            Some("--status-only") if args.next().is_none() => ("status", None, None, None),
            Some("--arm-four-timers-only")
                if args.next().as_deref() == Some("--experimental-four-timers")
                    && args.next().is_none() =>
            {
                ("arm", None, None, None)
            }
            Some("--set-game-preset-only") => {
                let preset = args.next().ok_or("missing selected-game speed preset")?;
                if !matches!(preset.as_str(), "half" | "normal" | "double")
                    || args.next().as_deref() != Some("--experimental-four-timers")
                    || args.next().is_some()
                {
                    return Err("invalid or unacknowledged selected-game speed preset".into());
                }
                ("preset", Some(preset), None, None)
            }
            Some("--set-game-rate-milli-only") => {
                let raw = args
                    .next()
                    .ok_or("missing selected-game rate in thousandths")?;
                let rate = raw
                    .parse::<u32>()
                    .map_err(|_| "invalid selected-game rate in thousandths")?;
                if !(100..=10_000).contains(&rate)
                    || raw != rate.to_string()
                    || args.next().as_deref() != Some("--experimental-four-timers")
                    || args.next().is_some()
                {
                    return Err(
                        "invalid or unacknowledged selected-game rate in thousandths".into(),
                    );
                }
                ("rate", None, Some(rate), None)
            }
            Some("--step-game-speed-only") => {
                let operation = args.next().ok_or("missing selected-game speed action")?;
                if !matches!(
                    operation.as_str(),
                    "increase" | "decrease" | "toggle" | "reset"
                ) || args.next().as_deref() != Some("--experimental-four-timers")
                    || args.next().is_some()
                {
                    return Err("invalid or unacknowledged selected-game speed action".into());
                }
                ("step", None, None, Some(operation))
            }
            _ => return Err("unrecognized game action flags".into()),
        };
        let target = AttachTarget::SelectedOfflineGame {
            path: PathBuf::from(expected),
            created_100ns,
        };
        match action {
            "stop" => shutdown_selected(pid, &target),
            "status" => status_selected(pid, &target),
            "arm" => arm_selected(pid, &target),
            "preset" => preset_selected(pid, &target, preset.as_deref().unwrap_or_default()),
            "rate" => rate_selected(pid, &target, milli_rate.unwrap_or_default()),
            "step" => step_selected(pid, &target, step_action.as_deref().unwrap_or_default()),
            _ => attach(pid, target),
        }
    }

    /// Enable experimental four-API timers only for the still-selected live
    /// offline target. The helper verifies the same identity again under its
    /// per-target mutex and rejects recognized protection modules. The DLL
    /// independently requires Running and an intact original IAT layout.
    fn arm_selected(pid: u32, target: &AttachTarget) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let dll = executable
            .parent()
            .ok_or("game helper has no directory")?
            .join("intimatr.dll");
        let process = own(
            unsafe { OpenProcess(ACCESS, 0, pid) },
            "cannot open selected game for timer activation",
        )?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        let machine = unsafe { running_process_architecture(process.0) }
            .ok_or("cannot verify selected game architecture before timer activation")?;
        if machine.architecture != "x86_64" {
            return Err("cannot activate timers in unsupported game architecture".into());
        }
        let _guard = acquire_attach_mutex(pid)?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        reject_observed_protection_modules(pid)?;
        if target_intimatr_module(pid, &dll)? != IntimatrModulePresence::Expected {
            return Err("cannot activate timers: exact owned Intimatr DLL is not loaded".into());
        }
        super::remote_status::arm_four_timers(process.0, pid, &dll)?;
        println!("INTIMATR_REMOTE_FOUR_TIMERS_INSTALLED=1");
        Ok(())
    }

    /// Set one bounded preset only in an actively armed selected game.
    /// Revalidate architecture, image, generation, loaded DLL and known
    /// protection modules under the same per-target mutex.
    fn preset_selected(pid: u32, target: &AttachTarget, preset: &str) -> Result<(), String> {
        apply_selected_rate(pid, target, Some(preset), None, None)
    }

    fn rate_selected(pid: u32, target: &AttachTarget, rate_milli: u32) -> Result<(), String> {
        apply_selected_rate(pid, target, None, Some(rate_milli), None)
    }

    fn step_selected(pid: u32, target: &AttachTarget, action: &str) -> Result<(), String> {
        apply_selected_rate(pid, target, None, None, Some(action))
    }

    fn apply_selected_rate(
        pid: u32,
        target: &AttachTarget,
        preset: Option<&str>,
        rate_milli: Option<u32>,
        step_action: Option<&str>,
    ) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let dll = executable
            .parent()
            .ok_or("game helper has no directory")?
            .join("intimatr.dll");
        let process = own(
            unsafe { OpenProcess(ACCESS, 0, pid) },
            "cannot open selected game for speed preset",
        )?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        let machine = unsafe { running_process_architecture(process.0) }
            .ok_or("cannot verify selected game architecture before speed preset")?;
        if machine.architecture != "x86_64" {
            return Err("cannot set speed in unsupported game architecture".into());
        }
        let _guard = acquire_attach_mutex(pid)?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        reject_observed_protection_modules(pid)?;
        if target_intimatr_module(pid, &dll)? != IntimatrModulePresence::Expected {
            return Err("cannot set speed: exact owned Intimatr DLL is not loaded".into());
        }
        let applied = match (preset, rate_milli, step_action) {
            (Some(preset), None, None) => {
                super::remote_status::select_game_preset(process.0, pid, &dll, preset)?
            }
            (None, Some(rate), None) => {
                super::remote_status::select_game_rate_milli(process.0, pid, &dll, rate)?
            }
            (None, None, Some(action)) => {
                super::remote_status::change_game_speed(process.0, pid, &dll, action)?
            }
            _ => return Err("selected-game rate request is ambiguous".into()),
        };
        println!("INTIMATR_REMOTE_GAME_RATE_MILLI={applied}");
        Ok(())
    }

    /// Shutdown only the exact owned DLL in the exact selected process.
    /// Rechecks path, process generation and architecture under its mutex.
    fn shutdown_selected(pid: u32, target: &AttachTarget) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let dll = executable
            .parent()
            .ok_or("game helper has no directory")?
            .join("intimatr.dll");
        let process = own(
            unsafe { OpenProcess(ACCESS, 0, pid) },
            "cannot open selected game for Intimatr shutdown",
        )?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        let machine = unsafe { running_process_architecture(process.0) }
            .ok_or("cannot verify game architecture before stopping Intimatr")?;
        if machine.architecture != "x86_64" {
            return Err("cannot stop Intimatr in unsupported game architecture".into());
        }
        let _guard = acquire_attach_mutex(pid)?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        if target_intimatr_module(pid, &dll)? != IntimatrModulePresence::Expected {
            return Err(
                "cannot stop: exact owned Intimatr DLL is not loaded in selected game".into(),
            );
        }
        super::remote_status::request_shutdown(process.0, pid, &dll)?;
        println!("INTIMATR_REMOTE_RUNTIME_STOPPED=1");
        Ok(())
    }

    /// Query one selected process without loading, stopping, or changing its DLL.
    /// A remote thread executes only Intimatr's bounded lifecycle query.
    fn status_selected(pid: u32, target: &AttachTarget) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let dll = executable
            .parent()
            .ok_or("game helper has no directory")?
            .join("intimatr.dll");
        let process = own(
            unsafe { OpenProcess(ACCESS, 0, pid) },
            "cannot open selected game for Intimatr status",
        )?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        let machine = unsafe { running_process_architecture(process.0) }
            .ok_or("cannot verify selected game architecture before status query")?;
        if machine.architecture != "x86_64" {
            return Err("cannot query Intimatr in unsupported game architecture".into());
        }
        let _guard = acquire_attach_mutex(pid)?;
        validate_target_image(process.0, &executable, target)?;
        validate_target_generation(process.0, target)?;
        let (state, hook_state, rate_milli, import_mask, profile_state, profile_sha256) =
            match target_intimatr_module(pid, &dll)? {
                IntimatrModulePresence::Absent => ("absent", "none", 0, None, "unavailable", None),
                IntimatrModulePresence::Expected => {
                    let state = super::remote_status::query_lifecycle_state(process.0, pid, &dll)?;
                    let hooks = super::remote_status::query_timer_hook_state(process.0, pid, &dll)?;
                    let rate = super::remote_status::query_rate_milli(process.0, pid, &dll)?;
                    let coverage =
                        super::remote_status::query_timer_import_coverage(process.0, pid, &dll)?;
                    let profile =
                        super::remote_status::query_explicit_profile_state(process.0, pid, &dll)?;
                    let digest =
                        super::remote_status::query_explicit_profile_sha256(process.0, pid, &dll)?;
                    if (profile == "accepted") != digest.is_some() {
                        return Err("target accepted profile and SHA-256 evidence disagree".into());
                    }
                    if (hooks == "installed_four_timers") != (rate != 0)
                        || (hooks == "installed_four_timers" && coverage != Some(15))
                    {
                        return Err(
                            "target timer hook state, imports and clock rate disagree".into()
                        );
                    }
                    (state, hooks, rate, coverage, profile, digest)
                }
                IntimatrModulePresence::UnexpectedPath => {
                    return Err("cannot query status of a different Intimatr DLL".into());
                }
            };
        println!("INTIMATR_REMOTE_RUNTIME_STATE={state}");
        println!("INTIMATR_REMOTE_TIMER_HOOK_STATE={hook_state}");
        println!("INTIMATR_REMOTE_GAME_RATE_MILLI={rate_milli}");
        println!("INTIMATR_REMOTE_SPEED_PROFILE_STATE={profile_state}");
        println!(
            "INTIMATR_REMOTE_SPEED_PROFILE_SHA256={}",
            profile_sha256.as_ref().map_or_else(
                || "unavailable".to_owned(),
                intimatr::speed_profile_fingerprint::sha256_hex,
            )
        );
        println!(
            "INTIMATR_REMOTE_TIMER_IMPORT_MASK={}",
            import_mask.map_or_else(|| "unknown".to_owned(), |mask| mask.to_string())
        );
        Ok(())
    }

    fn attach(pid: u32, target: AttachTarget) -> Result<(), String> {
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
        validate_target_image(process.0, &executable, &target)?;
        validate_target_generation(process.0, &target)?;
        // Independent from both its filename and on-disk PE headers: query
        // the already-running process on the held Windows handle. This x64
        // loader must never guess at a 32-bit or unknown target architecture.
        let machine = unsafe { running_process_architecture(process.0) }
            .ok_or("refusing attach: live process architecture is unavailable")?;
        if machine.architecture != "x86_64" {
            return Err(format!(
                "refusing attach: unsupported live process architecture {}",
                machine.architecture
            ));
        }
        let _attachment_guard = acquire_attach_mutex(pid)?;
        // Identity is rechecked under the same lock immediately before
        // any allocation or target memory writes.
        validate_target_image(process.0, &executable, &target)?;
        validate_target_generation(process.0, &target)?;
        if matches!(target, AttachTarget::SelectedOfflineGame { .. }) {
            reject_observed_protection_modules(pid)?;
        }
        if target_intimatr_module(pid, &dll)? != IntimatrModulePresence::Absent {
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
                if target_intimatr_module(pid, &dll)? != IntimatrModulePresence::Expected {
                    return Err(
                        "remote loader exited without the expected owned Intimatr DLL in target"
                            .into(),
                    );
                }
                if matches!(target, AttachTarget::SelectedOfflineGame { .. }) {
                    // Asynchronous DLL bootstrap may fail *after* LoadLibrary
                    // returns. Confirm the actual target runtime separately.
                    super::remote_status::wait_until_running(process.0, pid, &dll)?;
                    println!("INTIMATR_REMOTE_RUNTIME_RUNNING=1");
                }
                println!("Wine helper confirmed the exact Intimatr DLL in Windows PID {pid}");
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
        use super::{
            ERROR_NO_MORE_FILES, IntimatrModulePresence, classify_intimatr_module_path,
            complete_module_scan, same_owned_fixture_image,
        };
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

        #[test]
        fn loaded_module_requires_expected_owned_dll_path() {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "intimatr-module-identity-{}-{id}",
                std::process::id()
            ));
            let owned_dir = root.join("owned");
            let other_dir = root.join("other");
            fs::create_dir_all(&owned_dir).unwrap();
            fs::create_dir_all(&other_dir).unwrap();
            let expected = owned_dir.join("intimatr.dll");
            let unrelated = other_dir.join("intimatr.dll");
            fs::write(&expected, b"owned fixture module").unwrap();
            fs::write(&unrelated, b"same name, unrelated file").unwrap();

            assert_eq!(
                classify_intimatr_module_path(&expected, &expected).unwrap(),
                IntimatrModulePresence::Expected
            );
            assert_eq!(
                classify_intimatr_module_path(&unrelated, &expected).unwrap(),
                IntimatrModulePresence::UnexpectedPath
            );
            assert!(
                classify_intimatr_module_path(&other_dir.join("missing.dll"), &expected).is_err()
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[cfg(test)]
    mod game_arg_tests {
        use super::*;
        #[test]
        fn refuses_system_image_names_in_selection() {
            for name in ["explorer.exe", "rundll32.exe", "wine-process-list.exe"] {
                assert!(
                    game_path_key(&format!(r"C:\Windows\System32\{name}")).is_ok(),
                    "path validation is distinct from the live image denylist"
                );
            }
        }
    }
}
