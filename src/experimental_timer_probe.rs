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
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{
    speed_config::SpeedSettings,
    speed_session::{SpeedAction, SpeedSession},
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

struct ProbeHooks {
    session: SpeedSession,
    /// Valid only for the lifetime of the host executable (never freed here).
    slots: Vec<(usize, usize)>,
}

static HOOKS: OnceLock<Mutex<Option<ProbeHooks>>> = OnceLock::new();

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
    // Never unwind across the Windows ABI. Falling back to the real kernel32
    // import keeps a failed lock/initialization from corrupting the target.
    if value.is_null() {
        return 0;
    }
    let Some(real) = native_qpc() else {
        return 0;
    };
    let virtual_ticks = match hooks().try_lock() {
        Ok(mut guard) => guard
            .as_mut()
            .map(|active| active.session.sample(real).performance_counter)
            .unwrap_or(real),
        Err(_) => real,
    };
    unsafe { value.write(virtual_ticks.min(i64::MAX as u64) as i64) };
    1
}

fn get_qpc_address() -> Option<usize> {
    let kernel = unsafe { GetModuleHandleW(wide_literal_kernel32().as_ptr()) };
    if kernel.is_null() {
        return None;
    }
    let address = unsafe { GetProcAddress(kernel, c"QueryPerformanceCounter".as_ptr().cast()) };
    (!address.is_null()).then_some(address as usize)
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

fn qpc_import_slots() -> Option<Vec<(usize, usize)>> {
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

    let original = get_qpc_address()?;
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
            if resolved == original {
                slots.push((unsafe { image.add(thunk_offset) } as usize, resolved));
            }
        }
    }
    (!slots.is_empty()).then_some(slots)
}

/// Restore writable protection even if the compare/exchange fails. A slot
/// not owned by this hook is never overwritten.
unsafe fn patch_slot(slot: usize, expected: usize, replacement: usize) -> bool {
    if !slot.is_multiple_of(std::mem::align_of::<usize>()) {
        return false;
    }
    let mut previous = 0;
    if unsafe { VirtualProtect(slot as *mut c_void, 8, PAGE_READWRITE, &mut previous) } == 0 {
        return false;
    }
    let entry = unsafe { &*(slot as *const AtomicUsize) };
    let changed = entry
        .compare_exchange(expected, replacement, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let mut ignored = 0;
    let restored = unsafe { VirtualProtect(slot as *mut c_void, 8, previous, &mut ignored) } != 0;
    changed && restored
}

/// Only succeeds in Intimatr's fixture; never installs hooks into an
/// arbitrary game, anti-cheat process, or another Wine program.
pub fn enable_probe() -> bool {
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
    if guard.is_some() {
        return true;
    }

    let (Some(real_qpc), Some(slots)) = (native_qpc(), qpc_import_slots()) else {
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
        unsafe { GetTickCount() },
    ) else {
        return false;
    };
    let Some(now) = native_qpc() else {
        return false;
    };
    // Sample before hooking; the tracked timeline is continuous.
    session.sample(now);
    let replacement = intercepted_qpc as *const () as usize;
    let mut installed = Vec::new();
    for (slot, original) in slots {
        if !unsafe { patch_slot(slot, original, replacement) } {
            for (prior_slot, prior_value) in &installed {
                // Recovery best-effort; fail closed and retain code ownership
                // rather than pretending a partially patched process is safe.
                if !unsafe { patch_slot(*prior_slot, replacement, *prior_value) } {
                    return false;
                }
            }
            return false;
        }
        installed.push((slot, original));
    }
    *guard = Some(ProbeHooks {
        session,
        slots: installed,
    });
    true
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
    active
        .session
        .apply(real, SpeedAction::SelectPreset(multiplier))
        .is_ok()
}

pub fn probe_hook_count() -> u32 {
    hooks()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|active| active.slots.len() as u32))
        .unwrap_or(0)
}

pub fn disable_probe() -> bool {
    let Ok(mut guard) = hooks().lock() else {
        return false;
    };
    let Some(active) = guard.as_ref() else {
        return true;
    };
    for (slot, original) in &active.slots {
        if !unsafe { patch_slot(*slot, intercepted_qpc as *const () as usize, *original) } {
            return false;
        }
    }
    *guard = None;
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
