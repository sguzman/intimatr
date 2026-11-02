//! Experimental IAT clock hook, restricted to Intimatr's own offline Wine
//! test executable. NOT enabled during ordinary DLL bootstrap.
//!
//! The first iteration patches *only* the executable's import for QPC, not
//! arbitrary game modules or detours. It proves true Windows timing API
//! interception and fully restores its import before unloading the DLL.
//! Do not extend the allowlist until real-process lifecycle and concurrency
//! tests cover the new target type.

use std::{
    ffi::c_void,
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
};

use crate::{
    speed_config::SpeedSettings,
    speed_session::{SpeedAction, SpeedSession},
    speed_timers::TimerSnapshot,
};

const PAGE_READWRITE: u32 = 0x04;
const MAX_IMAGE_SIZE: usize = 512 * 1024 * 1024;
const MAX_DESCRIPTORS: usize = 256;
const MAX_THUNKS: usize = 8192;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
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
    active: bool,
}

struct ProbeHooks {
    session: SpeedSession,
    /// Each swapped import remains owned until positively restored.
    slots: Vec<HookSlot>,
}

static HOOKS: OnceLock<Mutex<Option<ProbeHooks>>> = OnceLock::new();

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

fn wide_literal_kernel32() -> &'static [u16] {
    const NAME: &[u16] = &[107, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0];
    NAME
}

/// Read a primitive at a checked relative offset in a mapped PE image.
/// Bounds do not protect against a maliciously remapped executable; the
/// prototype is intentionally confined to our own built test fixture.
unsafe fn read_u16(image: *const u8, size: usize, offset: usize) -> Option<u16> {
    (offset <= size.checked_sub(2)?)
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<u16>()) })
}
unsafe fn read_u32(image: *const u8, size: usize, offset: usize) -> Option<u32> {
    (offset <= size.checked_sub(4)?)
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<u32>()) })
}
unsafe fn read_usize(image: *const u8, size: usize, offset: usize) -> Option<usize> {
    (offset <= size.checked_sub(8)?)
        .then(|| unsafe { ptr::read_unaligned(image.add(offset).cast::<usize>()) })
}

fn timer_import_slots() -> Option<Vec<(usize, usize, usize)>> {
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
    if imports == 0 || import_size < 20 || imports.checked_add(import_size)? > size {
        return None;
    }

    let originals = original_timer_addresses()?;
    let mut slots = Vec::new();
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
            for (original, replacement) in originals {
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
    (!slots.is_empty()).then_some(slots)
}

/// Record the pointer mutation independently of page-protection recovery.
#[derive(Clone, Copy)]
struct PatchOutcome {
    swapped: bool,
    protection_restored: bool,
}

unsafe fn patch_slot(slot: usize, expected: usize, replacement: usize) -> PatchOutcome {
    let fail = PatchOutcome {
        swapped: false,
        protection_restored: false,
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
    }
}

/// A partial failed restoration keeps all remaining active slots owned by the
/// module so the host can retry instead of unloading live function pointers.
fn restore_slots(slots: &mut [HookSlot]) -> bool {
    let mut clean = true;
    for hook in slots.iter_mut().filter(|hook| hook.active) {
        let outcome = unsafe { patch_slot(hook.address, hook.replacement, hook.original) };
        if outcome.swapped {
            hook.active = false;
        } else {
            clean = false;
        }
        if !outcome.protection_restored {
            clean = false;
        }
    }
    clean && slots.iter().all(|hook| !hook.active)
}

/// Only succeeds in Intimatr's fixture; never installs hooks into an
/// arbitrary game, anti-cheat process, or another Wine program.
pub fn enable_probe() -> bool {
    enable_probe_with_fault(None)
}

/// Only the disposable Wine probe invokes the deterministic rollback fault.
fn enable_probe_with_fault(fail_after: Option<usize>) -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let is_our_probe = executable.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .eq_ignore_ascii_case("wine-load-probe.exe")
    });
    if !is_our_probe {
        return false;
    }
    let Ok(mut guard) = hooks().lock() else {
        return false;
    };
    if let Some(active) = guard.as_ref() {
        return active.slots.iter().filter(|slot| slot.active).count() >= 4;
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
        if result.swapped {
            installed.push(HookSlot {
                address: slot,
                original,
                replacement,
                active: true,
            });
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
    let Some(real) = native_qpc() else {
        return false;
    };
    if let Ok(snapshot) = active.session.apply(real, SpeedAction::SelectPreset(multiplier)) {
        publish(snapshot);
        true
    } else {
        false
    }
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
    *guard = None;
    VIRTUAL_READY.store(false, Ordering::Release);
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_enable() -> i32 {
    i32::from(enable_probe())
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_rate(multiplier: f64) -> i32 {
    i32::from(set_probe_rate(multiplier))
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_hook_count() -> u32 {
    probe_hook_count()
}
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_disable() -> i32 {
    i32::from(disable_probe())
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_timer_probe_fail_after(steps: u32) -> i32 {
    i32::from(enable_probe_with_fault(Some(steps as usize)))
}
