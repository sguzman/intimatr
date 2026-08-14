use std::{ffi::c_void, panic::AssertUnwindSafe};

use windows_sys::Win32::{
    Foundation::{BOOL, CloseHandle, HINSTANCE},
    System::{
        LibraryLoader::DisableThreadLibraryCalls,
        SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
        Threading::CreateThread,
    },
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
