//! Live Windows process architecture evidence shared by the Wine inventory
//! probe and the strictly allowlisted disposable remote-attach fixture.
//!
//! The PE file's machine type and its running process machine are distinct.
//! No legacy fallback is allowed to guess the process architecture when
//! IsWow64Process2 is unavailable or reports an unrecognized machine.

use std::ffi::c_void;

type Handle = *mut c_void;
type Wow64Process2 = unsafe extern "system" fn(Handle, *mut u16, *mut u16) -> i32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedProcessMachine {
    pub architecture: &'static str,
    pub source: &'static str,
}

fn classify_process_machine(process_machine: u16, native_machine: u16) -> Option<&'static str> {
    // IMAGE_FILE_MACHINE_UNKNOWN means not WoW64. Only then should the
    // native architecture replace the process machine reported by Windows.
    let machine = if process_machine == 0 {
        native_machine
    } else {
        process_machine
    };
    match machine {
        0x014c => Some("x86"),
        0x8664 => Some("x86_64"),
        0xaa64 => Some("aarch64"),
        _ => None,
    }
}

/// Requires PROCESS_QUERY_LIMITED_INFORMATION on the provided process handle.
/// Absence of this OS function, an API error, or unknown machine values is
/// represented as None and must not be treated as x86-64 eligibility.
pub fn running_process_architecture(handle: Handle) -> Option<ObservedProcessMachine> {
    if handle.is_null() {
        return None;
    }
    let kernel = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr().cast()) };
    if kernel.is_null() {
        return None;
    }
    let procedure = unsafe { GetProcAddress(kernel, c"IsWow64Process2".as_ptr().cast()) };
    if procedure.is_null() {
        return None;
    }
    let query = unsafe { std::mem::transmute::<*const c_void, Wow64Process2>(procedure) };
    let mut process_machine = 0_u16;
    let mut native_machine = 0_u16;
    if unsafe { query(handle, &mut process_machine, &mut native_machine) } == 0 {
        return None;
    }
    classify_process_machine(process_machine, native_machine).map(|architecture| {
        ObservedProcessMachine {
            architecture,
            source: "is_wow64_process2",
        }
    })
}

#[cfg(test)]
mod tests {
    use super::classify_process_machine;

    #[test]
    fn x64_native_process_is_not_guessed_from_unknown_machine() {
        assert_eq!(classify_process_machine(0, 0x8664), Some("x86_64"));
        assert_eq!(classify_process_machine(0, 0x014c), Some("x86"));
        assert_eq!(classify_process_machine(0, 0), None);
        assert_eq!(classify_process_machine(0, 0xffff), None);
    }

    #[test]
    fn wow64_process_machine_overrides_native_machine() {
        assert_eq!(classify_process_machine(0x014c, 0x8664), Some("x86"));
        assert_eq!(classify_process_machine(0x8664, 0xaa64), Some("x86_64"));
        assert_eq!(classify_process_machine(0xaa64, 0x8664), Some("aarch64"));
        assert_eq!(classify_process_machine(0xbeef, 0x8664), None);
    }
}
