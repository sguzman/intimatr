//! Windows process generation identity: GetProcessTimes creation FILETIME.
//!
//! A Windows PID can be recycled. The 100-ns UTC creation timestamp is
//! checked on the held process HANDLE before remote attachment. Unlike an
//! executable path, it distinguishes distinct process instances with the
//! same image. It is NOT a substitute for explicit offline-game permission.
#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[cfg(windows)]
impl FileTime {
    fn ticks(self) -> u64 {
        (u64::from(self.high) << 32) | u64::from(self.low)
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
}

/// Return the nonzero Windows FILETIME creation epoch for an open process.
/// Requires PROCESS_QUERY_LIMITED_INFORMATION; failure leaves it unknown.
///
/// # Safety
/// The caller must supply a valid live Windows process HANDLE.
#[cfg(windows)]
pub unsafe fn process_creation_time_100ns(process: *mut c_void) -> Option<u64> {
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let ok = unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    (ok != 0 && creation.ticks() != 0).then(|| creation.ticks())
}

#[cfg(all(test, windows))]
mod tests {
    use super::FileTime;

    #[test]
    fn combines_filetime_halves_without_precision_loss() {
        assert_eq!(
            FileTime {
                high: 0x1234_5678,
                low: 0x9abc_def0
            }
            .ticks(),
            0x1234_5678_9abc_def0
        );
    }
}
