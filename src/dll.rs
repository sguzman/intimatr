use std::{ffi::c_void, panic::AssertUnwindSafe};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HINSTANCE},
        System::{
            LibraryLoader::DisableThreadLibraryCalls,
            SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
            Threading::CreateThread,
        },
    },
    core::BOOL,
};

use crate::{platform::windows, runtime};

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    match std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        dll_main_inner(module, reason)
    })) {
        Ok(result) => result,
        Err(_) => {
            runtime::mark_failed();
            0
        }
    }
}

unsafe fn dll_main_inner(module: HINSTANCE, reason: u32) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH => {
            if runtime::mark_attached().is_err() {
                return 0;
            }

            unsafe {
                DisableThreadLibraryCalls(module);
            }

            let thread = unsafe {
                CreateThread(
                    std::ptr::null(),
                    0,
                    Some(bootstrap_thread_proc),
                    module as *const c_void,
                    0,
                    std::ptr::null_mut(),
                )
            };

            if thread.is_null() {
                runtime::mark_failed();
                return 0;
            }

            unsafe {
                CloseHandle(thread);
            }
            1
        }
        DLL_PROCESS_DETACH => {
            runtime::request_shutdown_from_loader();
            1
        }
        _ => 1,
    }
}

unsafe extern "system" fn bootstrap_thread_proc(parameter: *mut c_void) -> u32 {
    let module = parameter as HINSTANCE;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| runtime::bootstrap_windows(module)));

    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            runtime::mark_failed();
            windows::debug_output(&format!("Intimatr bootstrap failed: {error}\n"));
            1
        }
        Err(_) => {
            runtime::mark_failed();
            windows::debug_output("Intimatr bootstrap panicked before completion\n");
            2
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_lifecycle_state() -> u32 {
    runtime::lifecycle_state().as_u8() as u32
}

/// Side-effect-free lifecycle query for an explicitly selected remote game.
/// This export has the exact LPTHREAD_START_ROUTINE ABI so a bounded helper
/// thread can return the atomic state without pointer arguments or probing
/// arbitrary DLL exports. It never installs timing hooks or changes speed.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_lifecycle_query(_parameter: *mut c_void) -> u32 {
    runtime::lifecycle_state().as_u8() as u32
}

/// Side-effect-free status for the separately managed four-timer IAT backend.
/// An attached DLL can be Running while no timing hooks are installed.
/// The returned code never represents measured game-time acceleration.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_timer_hook_state(_parameter: *mut c_void) -> u32 {
    crate::experimental_timer_probe::remote_hook_state()
}

/// Explicit, opt-in selected-offline-game timer activation. Only the
/// dedicated exact-identity Windows helper invokes this remote-thread ABI.
/// It installs no hooks at bootstrap and starts from 1x, never an automatic
/// stored acceleration preset. Missing or inconsistent IAT imports fail closed.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_arm_four_timers(_parameter: *mut c_void) -> u32 {
    match std::panic::catch_unwind(AssertUnwindSafe(runtime::arm_selected_game_timers)) {
        Ok(true) => 1,
        Ok(false) | Err(_) => 0,
    }
}

/// Bounded, exact-target helper command. Selector is an integer encoded in
/// the remote thread argument and is never dereferenced as a pointer.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_game_preset(selector: *mut c_void) -> u32 {
    let rate = match selector as usize {
        1 => 0.5,
        2 => 1.0,
        3 => 2.0,
        _ => return 0,
    };
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        runtime::selected_game_select_rate(rate)
    })) {
        Ok(true) => 1,
        Ok(false) | Err(_) => 0,
    }
}

/// Installed virtual-clock rate, in thousandths; zero means unavailable.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_game_rate_milli(_parameter: *mut c_void) -> u32 {
    crate::experimental_timer_probe::remote_rate_milli()
}

/// ABI-compatible remote shutdown of this DLL's own runtime. It deliberately
/// does not call FreeLibrary or unload callback code.
#[unsafe(no_mangle)]
pub extern "system" fn intimatr_remote_shutdown(_parameter: *mut c_void) -> u32 {
    match std::panic::catch_unwind(AssertUnwindSafe(runtime::shutdown)) {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            windows::debug_output(&format!("Intimatr remote shutdown failed: {error}\n"));
            0
        }
        Err(_) => {
            windows::debug_output("Intimatr remote shutdown panicked\n");
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn intimatr_request_shutdown() -> BOOL {
    let result = std::panic::catch_unwind(AssertUnwindSafe(runtime::shutdown));

    match result {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            windows::debug_output(&format!("Intimatr shutdown failed: {error}\n"));
            0
        }
        Err(_) => {
            windows::debug_output("Intimatr shutdown panicked\n");
            0
        }
    }
}
