//! Experimental four-timer IAT clock hook. Never enabled at DLL bootstrap.
//! Ordinary games require a separate exact-identity, explicit offline-game
//! activation request after attachment, and unsupported import layouts fail
//! closed. Hook callback code remains pinned until process exit.
//! This is not a detour backend or universal game compatibility claim.

use std::{
    ffi::c_void,
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualQuery,
};

use crate::{
    lifecycle::LifecycleState,
    speed_config::SpeedSettings,
    speed_hook_lifecycle::CallbackTracker,
    speed_session::{SpeedAction, SpeedSession},
    speed_timers::TimerSnapshot,
};

const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x01;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x04;
const MAX_IMAGE_SIZE: usize = 512 * 1024 * 1024;
const MAX_DESCRIPTORS: usize = 256;
const MAX_THUNKS: usize = 8192;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn GetModuleHandleExW(flags: u32, name: *const u16, result: *mut *mut c_void) -> i32;
    fn QueryPerformanceCounter(value: *mut i64) -> i32;
    fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    fn GetTickCount64() -> u64;
    fn GetTickCount() -> u32;
    fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeGetTime() -> u32;
}

#[derive(Debug, Clone, Copy)]
struct HookSlot {
    address: usize,
    original: usize,
    replacement: usize,
    /// Page protection before this fixture modified the import. Retained
    /// even when the pointer itself has been successfully restored.
    original_protection: u32,
    active: bool,
    protection_restored: bool,
}

struct ProbeHooks {
    session: SpeedSession,
    /// Each swapped import remains owned until positively restored.
    slots: Vec<HookSlot>,
}

static HOOKS: OnceLock<Mutex<Option<ProbeHooks>>> = OnceLock::new();
static ACTIVE_CALLBACKS: CallbackTracker = CallbackTracker::new();
// A PIN is intentionally irreversible until process exit. It prevents a
// previously-fetched IAT function pointer from calling unmapped DLL code.
static MODULE_PINNED: AtomicBool = AtomicBool::new(false);

// Published only while owning HOOKS. Contended API reads return the last
// scaled sample rather than a raw counter that could jump backwards at 2x.
static VIRTUAL_READY: AtomicBool = AtomicBool::new(false);
static LAST_QPC: AtomicU64 = AtomicU64::new(0);
static LAST_TICK64: AtomicU64 = AtomicU64::new(0);
static LAST_TICK32: AtomicU32 = AtomicU32::new(0);
static LAST_WINMM: AtomicU32 = AtomicU32::new(0);

fn publish(snapshot: TimerSnapshot) {
    LAST_QPC.store(snapshot.performance_counter, Ordering::Relaxed);
    LAST_TICK64.store(snapshot.tick_count64, Ordering::Relaxed);
    LAST_TICK32.store(snapshot.tick_count, Ordering::Relaxed);
    LAST_WINMM.store(snapshot.time_get_time, Ordering::Relaxed);
    VIRTUAL_READY.store(true, Ordering::Release);
}

fn last_or<T: Copy>(value: &impl LastValue<T>, real: T) -> T {
    if VIRTUAL_READY.load(Ordering::Acquire) {
        value.read_last()
    } else {
        real
    }
}

trait LastValue<T> {
    fn read_last(&self) -> T;
}

impl LastValue<u64> for AtomicU64 {
    fn read_last(&self) -> u64 {
        self.load(Ordering::Relaxed)
    }
}

impl LastValue<u32> for AtomicU32 {
    fn read_last(&self) -> u32 {
        self.load(Ordering::Relaxed)
    }
}

fn hooks() -> &'static Mutex<Option<ProbeHooks>> {
    HOOKS.get_or_init(|| Mutex::new(None))
}

fn native_qpc() -> Option<u64> {
    let mut value = 0_i64;
    if unsafe { QueryPerformanceCounter(&mut value) } == 0 {
        return None;
    }
    u64::try_from(value).ok()
}

unsafe extern "system" fn intercepted_qpc(value: *mut i64) -> i32 {
    let _callback = ACTIVE_CALLBACKS.enter();
    if value.is_null() {
        return 0;
    }
    let Some(real) = native_qpc() else {
        return 0;
    };
    let scaled = match hooks().try_lock() {
        Ok(mut guard) => {
            if let Some(active) = guard.as_mut() {
                let sample = active.session.sample(real);
                publish(sample);
                sample.performance_counter
            } else {
                last_or(&LAST_QPC, real)
            }
        }
        Err(_) => last_or(&LAST_QPC, real),
    };
    unsafe { value.write(scaled.min(i64::MAX as u64) as i64) };
    1
}

unsafe extern "system" fn intercepted_tick_count64() -> u64 {
    let _callback = ACTIVE_CALLBACKS.enter();
    let Some(real) = native_qpc() else {
        return unsafe { GetTickCount64() };
    };
    match hooks().try_lock() {
        Ok(mut guard) => {
            if let Some(active) = guard.as_mut() {
                let sample = active.session.sample(real);
                publish(sample);
                sample.tick_count64
            } else {
                last_or(&LAST_TICK64, unsafe { GetTickCount64() })
            }
        }
        Err(_) => last_or(&LAST_TICK64, unsafe { GetTickCount64() }),
    }
}

unsafe extern "system" fn intercepted_tick_count() -> u32 {
    let _callback = ACTIVE_CALLBACKS.enter();
    let Some(real) = native_qpc() else {
        return unsafe { GetTickCount() };
    };
    match hooks().try_lock() {
        Ok(mut guard) => {
            if let Some(active) = guard.as_mut() {
                let sample = active.session.sample(real);
                publish(sample);
                sample.tick_count
            } else {
                last_or(&LAST_TICK32, unsafe { GetTickCount() })
            }
        }
        Err(_) => last_or(&LAST_TICK32, unsafe { GetTickCount() }),
    }
}

unsafe extern "system" fn intercepted_time_get_time() -> u32 {
    let _callback = ACTIVE_CALLBACKS.enter();
    let Some(real) = native_qpc() else {
        return unsafe { timeGetTime() };
    };
    match hooks().try_lock() {
        Ok(mut guard) => {
            if let Some(active) = guard.as_mut() {
                let sample = active.session.sample(real);
                publish(sample);
                sample.time_get_time
            } else {
                last_or(&LAST_WINMM, unsafe { timeGetTime() })
            }
        }
        Err(_) => last_or(&LAST_WINMM, unsafe { timeGetTime() }),
    }
}

fn wide_literal_winmm() -> &'static [u16] {
    const NAME: &[u16] = &[119, 105, 110, 109, 109, 46, 100, 108, 108, 0];
    NAME
}

fn original_timer_addresses() -> Option<[(usize, usize); 4]> {
    let kernel = unsafe { GetModuleHandleW(wide_literal_kernel32().as_ptr()) };
    if kernel.is_null() {
        return None;
    }
    let imports = [
        (
            c"QueryPerformanceCounter",
            intercepted_qpc as *const () as usize,
        ),
        (
            c"GetTickCount64",
            intercepted_tick_count64 as *const () as usize,
        ),
        (
            c"GetTickCount",
            intercepted_tick_count as *const () as usize,
        ),
    ];
    let mut results = [(0, 0); 4];
    for (index, (name, replacement)) in imports.iter().enumerate() {
        let address = unsafe { GetProcAddress(kernel, name.as_ptr().cast()) };
        if address.is_null() {
            return None;
        }
        results[index] = (address as usize, *replacement);
    }
    let winmm = unsafe { GetModuleHandleW(wide_literal_winmm().as_ptr()) };
    if winmm.is_null() {
        return None;
    }
    let address = unsafe { GetProcAddress(winmm, c"timeGetTime".as_ptr().cast()) };
    if address.is_null() {
        return None;
    }
    results[3] = (
        address as usize,
        intercepted_time_get_time as *const () as usize,
    );
    Some(results)
}

/// A count of four slots is not proof that four distinct timer APIs are
/// intercepted: one API may have multiple import entries. Verify coverage
/// before any IAT pointer changes are attempted.
fn has_all_required_timer_imports(
    slots: &[(usize, usize, usize)],
    originals: &[(usize, usize); 4],
) -> bool {
    originals.iter().all(|&(original, replacement)| {
        slots
            .iter()
            .any(|&(_, found, hook)| found == original && hook == replacement)
    })
}

/// Partial rollback may leave several hooks installed. Such a state must
/// never be reported as an operational four-API backend.
fn fully_installed_hook_set(slots: &[HookSlot]) -> bool {
    let required = [
        intercepted_qpc as *const () as usize,
        intercepted_tick_count64 as *const () as usize,
        intercepted_tick_count as *const () as usize,
        intercepted_time_get_time as *const () as usize,
    ];
    !slots.is_empty()
        && slots
            .iter()
            .all(|slot| slot.active && slot.protection_restored)
        && required
            .into_iter()
            .all(|replacement| slots.iter().any(|slot| slot.replacement == replacement))
}

fn wide_literal_kernel32() -> &'static [u16] {
    const NAME: &[u16] = &[107, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0];
    NAME
}

/// Import parsing is confined to readable, committed PE pages; the image
/// SizeOfImage bound alone is not proof that every subrange remains mapped.
/// A concurrently unmapped executable can still race this check, so this
/// backend is experimental and fails closed on incompatible import layouts.
fn readable_mapped_range(address: *const u8, length: usize) -> bool {
    let mut region: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        VirtualQuery(
            address.cast(),
            &mut region,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
    {
        return false;
    }
    let allowed = matches!(
        region.Protect & 0xff,
        PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY
    );
    let start = region.BaseAddress as usize;
    let at = address as usize;
    region.State == MEM_COMMIT
        && allowed
        && region.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0
        && start <= at
        && at.checked_add(length).is_some_and(|end| {
            start
                .checked_add(region.RegionSize)
                .is_some_and(|limit| end <= limit)
        })
}

unsafe fn read_u16(image: *const u8, size: usize, offset: usize) -> Option<u16> {
    (offset <= size.checked_sub(2)? && readable_mapped_range(unsafe { image.add(offset) }, 2))
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<u16>()) })
}
unsafe fn read_u32(image: *const u8, size: usize, offset: usize) -> Option<u32> {
    (offset <= size.checked_sub(4)? && readable_mapped_range(unsafe { image.add(offset) }, 4))
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<u32>()) })
}
unsafe fn read_usize(image: *const u8, size: usize, offset: usize) -> Option<usize> {
    (offset <= size.checked_sub(8)? && readable_mapped_range(unsafe { image.add(offset) }, 8))
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<usize>()) })
}

/// Read-only observation of the main executable's current IAT. A complete
/// bitmask only establishes presence of these four imports, not that the
/// engine uses them for simulation timing.
struct TimerImportScan {
    slots: Vec<(usize, usize, usize)>,
    coverage_mask: u8,
}

fn scan_timer_imports() -> Option<TimerImportScan> {
    let image = unsafe { GetModuleHandleW(ptr::null()) }.cast::<u8>();
    if image.is_null() {
        return None;
    }
    // Validate the small PE headers before treating SizeOfImage as a bound.
    let header_size = 4096;
    if unsafe { read_u16(image, header_size, 0) }? != 0x5A4D {
        return None;
    }
    let pe = unsafe { read_u32(image, header_size, 0x3c) }? as usize;
    if pe > 2048 || unsafe { read_u32(image, header_size, pe) }? != 0x0000_4550 {
        return None;
    }
    let optional = pe.checked_add(24)?;
    if unsafe { read_u16(image, header_size, optional) }? != 0x20b {
        // x86-64 PE32+ only for this constrained experiment.
        return None;
    }
    let size = unsafe { read_u32(image, header_size, optional + 56) }? as usize;
    if !(4096..=MAX_IMAGE_SIZE).contains(&size) {
        return None;
    }
    // PE32+ import data-directory index 1, relative to OptionalHeader.
    let imports = unsafe { read_u32(image, header_size, optional + 120) }? as usize;
    let import_size = unsafe { read_u32(image, header_size, optional + 124) }? as usize;
    if imports == 0 && import_size == 0 {
        return Some(TimerImportScan {
            slots: Vec::new(),
            coverage_mask: 0,
        });
    }
    if imports == 0 || import_size < 20 || imports.checked_add(import_size)? > size {
        return None;
    }

    let originals = original_timer_addresses()?;
    let mut slots = Vec::new();
    let mut coverage_mask = 0_u8;
    for desc in 0..MAX_DESCRIPTORS {
        let offset = imports.checked_add(desc * 20)?;
        if offset.checked_add(20)? > imports + import_size {
            break;
        }
        let first_thunk = unsafe { read_u32(image, size, offset + 16) }? as usize;
        if first_thunk == 0 {
            break;
        }
        for i in 0..MAX_THUNKS {
            let thunk_offset = first_thunk.checked_add(i * 8)?;
            let resolved = unsafe { read_usize(image, size, thunk_offset) }?;
            if resolved == 0 {
                break;
            }
            for (index, &(original, replacement)) in originals.iter().enumerate() {
                if resolved == original || resolved == replacement {
                    coverage_mask |= 1_u8 << index;
                }
                if resolved == original {
                    slots.push((
                        unsafe { image.add(thunk_offset) } as usize,
                        resolved,
                        replacement,
                    ));
                }
            }
        }
    }
    Some(TimerImportScan {
        slots,
        coverage_mask,
    })
}

fn timer_import_slots() -> Option<Vec<(usize, usize, usize)>> {
    let scan = scan_timer_imports()?;
    let originals = original_timer_addresses()?;
    (scan.coverage_mask == 0x0f && has_all_required_timer_imports(&scan.slots, &originals))
        .then_some(scan.slots)
}

/// Read-only main-image timer IAT coverage. Bits: QPC=1, Tick64=2,
/// Tick32=4, WinMM=8. 0xffff_ffff represents unavailable/ambiguous parsing,
/// including partial rollback. It does not mutate imports or enable hooks.
pub fn remote_import_coverage_mask() -> u32 {
    match hooks().lock() {
        Ok(guard) => match guard.as_ref() {
            Some(active)
                if fully_installed_hook_set(&active.slots)
                    && MODULE_PINNED.load(Ordering::Acquire)
                    && !crate::runtime::shutdown_requested()
                    && crate::runtime::lifecycle_state() == LifecycleState::Running =>
            {
                return 0x0f;
            }
            Some(_) => return u32::MAX,
            None => {}
        },
        Err(_) => return u32::MAX,
    };
    scan_timer_imports().map_or(u32::MAX, |scan| u32::from(scan.coverage_mask))
}

/// Record the pointer mutation independently of page-protection recovery.
#[derive(Clone, Copy)]
struct PatchOutcome {
    swapped: bool,
    protection_restored: bool,
    /// None means the first VirtualProtect did not report a successful
    /// protection change, so the operation has no page state to repair.
    original_protection: Option<u32>,
}

unsafe fn patch_slot(slot: usize, expected: usize, replacement: usize) -> PatchOutcome {
    let fail = PatchOutcome {
        swapped: false,
        protection_restored: false,
        original_protection: None,
    };
    if !slot.is_multiple_of(std::mem::align_of::<usize>()) {
        return fail;
    }
    let mut previous = 0;
    if unsafe { VirtualProtect(slot as *mut c_void, 8, PAGE_READWRITE, &mut previous) } == 0 {
        return fail;
    }
    let entry = unsafe { &*(slot as *const AtomicUsize) };
    let swapped = entry
        .compare_exchange(expected, replacement, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let mut ignored = 0;
    let protection_restored =
        unsafe { VirtualProtect(slot as *mut c_void, 8, previous, &mut ignored) } != 0;
    PatchOutcome {
        swapped,
        protection_restored,
        original_protection: Some(previous),
    }
}

/// Keep pointer restoration and page-protection restoration as separate,
/// retryable obligations. A successful CAS does not imply that the IAT page
/// is back at its original protection, even after a later disable call.
/// Injecting the two operations permits deterministic fault-path tests.
fn restore_slots_with(
    slots: &mut [HookSlot],
    mut restore_pointer: impl FnMut(usize, usize, usize) -> PatchOutcome,
    mut restore_page: impl FnMut(usize, u32) -> bool,
) -> bool {
    let mut clean = true;
    for hook in slots
        .iter_mut()
        .filter(|hook| hook.active || !hook.protection_restored)
    {
        if hook.active {
            let outcome = restore_pointer(hook.address, hook.replacement, hook.original);
            if outcome.swapped {
                hook.active = false;
            } else {
                clean = false;
            }
        }
        // Always restore the protection captured BEFORE the first patch.
        // A failed earlier VirtualProtect could otherwise make a subsequent
        // patch report PAGE_READWRITE as its "original" protection.
        hook.protection_restored = restore_page(hook.address, hook.original_protection);
        if !hook.protection_restored {
            clean = false;
        }
    }
    clean
        && slots
            .iter()
            .all(|hook| !hook.active && hook.protection_restored)
}

/// Partial failures keep the hook record until both pointer and original
/// page protections have been positively restored. Never clear ownership
/// solely because all patched IAT pointers were replaced.
fn restore_slots(slots: &mut [HookSlot]) -> bool {
    restore_slots_with(
        slots,
        |address, expected, replacement| unsafe { patch_slot(address, expected, replacement) },
        |address, original_protection| {
            let mut ignored = 0;
            unsafe {
                VirtualProtect(address as *mut c_void, 8, original_protection, &mut ignored) != 0
            }
        },
    )
}

/// Win32 process-lifetime code ownership is required before the first IAT
/// pointer can refer to this DLL. An unsuccessful pin refuses installation.
/// The remote selected-game entry is gated by the exact-target helper.
fn pin_timer_module() -> bool {
    if MODULE_PINNED.load(Ordering::Acquire) {
        return true;
    }
    let mut module = ptr::null_mut();
    let address = intercepted_qpc as *const () as *const u16;
    let pinned = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            address,
            &mut module,
        )
    } != 0;
    if pinned && !module.is_null() {
        MODULE_PINNED.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

fn is_allowlisted_probe_process() -> bool {
    std::env::current_exe().ok().is_some_and(|executable| {
        executable.file_name().is_some_and(|name| {
            let file = name.to_string_lossy();
            file.eq_ignore_ascii_case("wine-load-probe.exe")
                || file.eq_ignore_ascii_case("wine-attach-target.exe")
        })
    })
}

/// Test-only direct activation; production games cannot call this entry
/// through the ordinary fixture interface.
pub fn enable_probe() -> bool {
    enable_probe_with_fault(None)
}

/// Called only by the dedicated selected-game remote control export after
/// identity, architecture, loaded-module and protection checks in the helper.
/// This path starts at the safe 1x baseline and still requires all four
/// native IAT timer imports in the executable's main PE image.
pub fn enable_selected_game_timers() -> bool {
    enable_hook_set_with_fault(None)
}

/// Only the disposable Wine probe invokes the deterministic rollback fault.
fn enable_probe_with_fault(fail_after: Option<usize>) -> bool {
    is_allowlisted_probe_process() && enable_hook_set_with_fault(fail_after)
}

fn enable_hook_set_with_fault(fail_after: Option<usize>) -> bool {
    let Ok(mut guard) = hooks().lock() else {
        return false;
    };
    // Check while holding the hook lock: shutdown requests the gate before
    // acquiring this same lock for restoration. Re-activation after a
    // completed or in-progress shutdown must never resurrect IAT callbacks.
    if crate::runtime::shutdown_requested()
        || crate::runtime::lifecycle_state() != LifecycleState::Running
    {
        return false;
    }
    if let Some(active) = guard.as_ref() {
        return fully_installed_hook_set(&active.slots);
    }

    let (Some(real_qpc), Some(slots)) = (native_qpc(), timer_import_slots()) else {
        return false;
    };
    let mut frequency = 0_i64;
    if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
        return false;
    }
    let Ok(mut session) = SpeedSession::new(
        &SpeedSettings {
            enabled: true,
            ..SpeedSettings::default()
        },
        real_qpc,
        frequency as u64,
        unsafe { GetTickCount64() },
        unsafe { timeGetTime() },
    ) else {
        return false;
    };
    let Some(now) = native_qpc() else {
        return false;
    };
    // Keep callback code mapped through process exit, even if another thread
    // fetched an import pointer before the restore barrier. No IAT mutation
    // is allowed unless this irreversible protection succeeds first.
    if !pin_timer_module() {
        return false;
    }
    // Sample before hooking; the tracked timeline is continuous.
    let baseline = session.sample(now);
    publish(baseline);
    let mut installed = Vec::with_capacity(slots.len());
    let mut complete = true;
    for (slot, original, replacement) in slots {
        if fail_after.is_some_and(|limit| installed.len() == limit) {
            complete = false;
            break;
        }
        let result = unsafe { patch_slot(slot, original, replacement) };
        if let Some(original_protection) = result.original_protection {
            // Also retain an UNSWAPPED slot whose page permissions were not
            // restored, rather than silently discarding failed cleanup.
            if result.swapped || !result.protection_restored {
                installed.push(HookSlot {
                    address: slot,
                    original,
                    replacement,
                    original_protection,
                    active: result.swapped,
                    protection_restored: result.protection_restored,
                });
            }
        }
        if !result.swapped || !result.protection_restored {
            complete = false;
            break;
        }
    }
    if complete {
        *guard = Some(ProbeHooks {
            session,
            slots: installed,
        });
        true
    } else {
        let clean = restore_slots(&mut installed);
        if !clean {
            // Retain DLL ownership if any import could still call this code.
            *guard = Some(ProbeHooks {
                session,
                slots: installed,
            });
        } else {
            VIRTUAL_READY.store(false, Ordering::Release);
        }
        false
    }
}

pub fn set_probe_rate(multiplier: f64) -> bool {
    let Ok(mut guard) = hooks().lock() else {
        return false;
    };
    let Some(active) = guard.as_mut() else {
        return false;
    };
    if !fully_installed_hook_set(&active.slots) {
        return false;
    }
    let Some(real) = native_qpc() else {
        return false;
    };
    if let Ok(snapshot) = active
        .session
        .apply(real, SpeedAction::SelectPreset(multiplier))
    {
        publish(snapshot);
        true
    } else {
        false
    }
}

/// The shared command executor may synchronize rate changes only while
/// an explicitly allowlisted disposable test process already has all four
/// IAT imports installed. This never authorizes hook installation.
/// Apply the original user action to the already-installed fixture hooks.
/// Unlike replaying a raw multiplier, an active hold retains its lease
/// deadline; repeated status reads cannot restart that deadline.
pub(crate) fn apply_installed_command(action: SpeedAction) -> Result<(), &'static str> {
    let mut guard = hooks().lock().map_err(|_| "timer hook state poisoned")?;
    let Some(active) = guard.as_mut() else {
        return Ok(());
    };
    if !fully_installed_hook_set(&active.slots)
        || crate::runtime::shutdown_requested()
        || crate::runtime::lifecycle_state() != LifecycleState::Running
    {
        return Err("installed timer hooks are unavailable or incomplete");
    }
    let real = native_qpc().ok_or("timer QPC unavailable")?;
    let snapshot = active
        .session
        .apply(real, action)
        .map_err(|_| "fixture clock rejected shared speed action")?;
    publish(snapshot);
    Ok(())
}

pub fn probe_module_pinned() -> bool {
    MODULE_PINNED.load(Ordering::Acquire)
}

/// Side-effect-free, bounded status for an explicitly selected target.
/// 0 = no owned IAT hooks, 1 = all four installed while runtime is Running,
/// 2 = partial/cleanup-pending, 3 = poisoned/unavailable. Never infer that
/// actual gameplay speed has changed solely from the installed-hook state.
pub fn remote_hook_state() -> u32 {
    match hooks().lock() {
        Ok(guard) => match guard.as_ref() {
            None => 0,
            Some(active)
                if fully_installed_hook_set(&active.slots)
                    && MODULE_PINNED.load(Ordering::Acquire)
                    && !crate::runtime::shutdown_requested()
                    && crate::runtime::lifecycle_state() == LifecycleState::Running =>
            {
                1
            }
            Some(_) => 2,
        },
        Err(_) => 3,
    }
}

/// Actual IAT virtual-clock session rate in thousandths of 1x. Zero
/// denotes an unavailable or incomplete hook set, never game FPS.
pub fn remote_rate_milli() -> u32 {
    if crate::runtime::shutdown_requested()
        || crate::runtime::lifecycle_state() != LifecycleState::Running
    {
        return 0;
    }
    let Ok(guard) = hooks().lock() else {
        return 0;
    };
    let Some(active) = guard.as_ref() else {
        return 0;
    };
    if !fully_installed_hook_set(&active.slots) || !MODULE_PINNED.load(Ordering::Acquire) {
        return 0;
    }
    (active.session.multiplier() * 1000.0).round() as u32
}

fn probe_hook_set_complete() -> bool {
    hooks()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|active| fully_installed_hook_set(&active.slots))
        })
        .unwrap_or(false)
}

/// A zero hook count alone does not prove successful cleanup: page
/// protections may still need repair, or callbacks may still be draining.
/// A poisoned lock is conservatively reported as pending.
pub fn probe_cleanup_pending() -> bool {
    hooks().lock().map_or(true, |guard| guard.is_some())
}

pub fn probe_hook_count() -> u32 {
    hooks()
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|active| active.slots.iter().filter(|slot| slot.active).count() as u32)
        })
        .unwrap_or(0)
}

pub fn disable_probe() -> bool {
    let Ok(mut guard) = hooks().lock() else {
        return false;
    };
    let Some(active) = guard.as_mut() else {
        return true;
    };
    if !restore_slots(&mut active.slots) {
        return false;
    }
    // Restoration prevents new normal IAT dispatches, but existing hook
    // calls may still be executing. Never report clean shutdown while a
    // tracked callback is active; keep ownership for a later retry.
    if !ACTIVE_CALLBACKS.wait_until_idle(Duration::from_millis(500)) {
        return false;
    }
    *guard = None;
    VIRTUAL_READY.store(false, Ordering::Release);
    true
}

/// Test-only bridge: apply a speed preset through the very same command
/// executor used by the in-process UI/RPC, then synchronize the experimental
/// four-API timer backend. It never installs hooks and never targets games.
/// Invalid/absent preview profiles or inactive fixture hooks fail closed.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_shared_rate(multiplier: f64) -> i32 {
    if !is_allowlisted_probe_process() || !probe_hook_set_complete() {
        return 0;
    }
    // The shared executor already applies this validated action to the
    // installed, fixture-only IAT clock. Reapplying the preset here would
    // unnecessarily reset its timer epoch twice.
    match crate::runtime::fixture_preview_select_rate(multiplier) {
        Some(selected) if selected == multiplier => 1,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_enable() -> i32 {
    i32::from(enable_probe())
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_rate(multiplier: f64) -> i32 {
    i32::from(is_allowlisted_probe_process() && set_probe_rate(multiplier))
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_module_pinned() -> i32 {
    i32::from(probe_module_pinned())
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_hook_count() -> u32 {
    probe_hook_count()
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_cleanup_pending() -> i32 {
    i32::from(probe_cleanup_pending())
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_disable() -> i32 {
    i32::from(disable_probe())
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_fail_after(steps: u32) -> i32 {
    i32::from(enable_probe_with_fault(Some(steps as usize)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_mask_tracks_each_timer_import_independently() {
        let originals = [(11_usize, 21_usize), (12, 22), (13, 23), (14, 24)];
        let coverage = |observed: &[usize]| {
            originals
                .iter()
                .enumerate()
                .fold(0_u8, |mask, (index, (original, replacement))| {
                    if observed.contains(original) || observed.contains(replacement) {
                        mask | (1_u8 << index)
                    } else {
                        mask
                    }
                })
        };
        assert_eq!(coverage(&[]), 0);
        assert_eq!(coverage(&[11, 12]), 0b0011);
        assert_eq!(coverage(&[14, 13, 12, 11]), 0b1111);
        assert_eq!(coverage(&[21, 22, 23, 24]), 0b1111);
        assert_eq!(coverage(&[11, 13, 24]), 0b1101);
    }

    #[test]
    fn import_coverage_requires_each_distinct_timer_api() {
        let originals = [(10, 110), (20, 120), (30, 130), (40, 140)];
        let complete = [(1, 10, 110), (2, 20, 120), (3, 30, 130), (4, 40, 140)];
        assert!(has_all_required_timer_imports(&complete, &originals));

        // Four entries can all reference the same API. Mere counting is unsafe.
        let duplicate_qpc = [(1, 10, 110), (2, 10, 110), (3, 10, 110), (4, 10, 110)];
        assert!(!has_all_required_timer_imports(&duplicate_qpc, &originals));

        let wrong_replacement = [(1, 10, 110), (2, 20, 120), (3, 30, 130), (4, 40, 999)];
        assert!(!has_all_required_timer_imports(
            &wrong_replacement,
            &originals
        ));
        assert!(!has_all_required_timer_imports(&complete[..3], &originals));
    }

    #[test]
    fn page_restore_failure_survives_pointer_rollback_and_can_be_retried() {
        let mut slots = [HookSlot {
            address: 8,
            original: 100,
            replacement: 200,
            original_protection: 0x02,
            active: true,
            protection_restored: true,
        }];
        let mut swaps = 0;
        // The IAT pointer was restored, but VirtualProtect failed afterward.
        assert!(!restore_slots_with(
            &mut slots,
            |_, _, _| {
                swaps += 1;
                PatchOutcome {
                    swapped: true,
                    protection_restored: false,
                    original_protection: Some(PAGE_READWRITE),
                }
            },
            |_, original| {
                assert_eq!(original, 0x02);
                false
            }
        ));
        assert_eq!(swaps, 1);
        assert!(!slots[0].active);
        assert!(!slots[0].protection_restored);

        // No second pointer swap is needed: only the stored original page
        // protection is retried. A false clean-shutdown report is avoided.
        assert!(restore_slots_with(
            &mut slots,
            |_, _, _| panic!("must not touch already-restored IAT pointer"),
            |_, original| original == 0x02
        ));
        assert!(!slots[0].active);
        assert!(slots[0].protection_restored);
    }

    #[test]
    fn failed_install_protection_restore_stays_owned_without_pointer_swap() {
        let mut slots = [HookSlot {
            address: 8,
            original: 100,
            replacement: 200,
            original_protection: 0x02,
            active: false,
            protection_restored: false,
        }];
        assert!(!fully_installed_hook_set(&slots));
        assert!(restore_slots_with(
            &mut slots,
            |_, _, _| panic!("failed install never swapped pointer"),
            |_, protection| protection == 0x02
        ));
    }

    #[test]
    fn partial_hook_rollback_never_looks_like_full_activation() {
        let required = [
            intercepted_qpc as *const () as usize,
            intercepted_tick_count64 as *const () as usize,
            intercepted_tick_count as *const () as usize,
            intercepted_time_get_time as *const () as usize,
        ];
        let mut slots: Vec<HookSlot> = required
            .into_iter()
            .enumerate()
            .map(|(index, replacement)| HookSlot {
                address: index + 1,
                original: index + 11,
                replacement,
                original_protection: 0x02,
                active: true,
                protection_restored: true,
            })
            .collect();
        assert!(fully_installed_hook_set(&slots));

        slots[3].active = false;
        assert!(!fully_installed_hook_set(&slots));
        slots[3].active = true;
        slots[3].replacement = slots[0].replacement;
        assert!(!fully_installed_hook_set(&slots));
        assert!(!fully_installed_hook_set(&[]));
    }
}
