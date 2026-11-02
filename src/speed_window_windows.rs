//! Conservative Win32 window enumeration for an in-process, offline game.
//! The caller must still explicitly opt in to speed input; discovery itself
//! only reports candidate handles and never polls or injects keyboard events.

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
    },
};

use crate::speed_window::{SelectionError, WindowCandidate, choose_unique};

const MAX_TITLE_UNITS: usize = 256;

unsafe extern "system" fn collect_window(hwnd: HWND, param: LPARAM) -> i32 {
    // SAFETY: enumerate_owned_windows passes a valid, uniquely borrowed
    // mutable Vec for the duration of the synchronous EnumWindows call.
    let result = unsafe { &mut *(param as *mut Vec<WindowCandidate>) };
    let mut process = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process) };
    if process != unsafe { GetCurrentProcessId() } {
        return 1;
    }
    let mut title = [0u16; MAX_TITLE_UNITS + 1];
    let size = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
    let text = String::from_utf16_lossy(&title[..size.max(0) as usize]);
    result.push(WindowCandidate {
        id: hwnd as usize as u64,
        current_process: true,
        top_level: true, // EnumWindows enumerates top-level windows.
        visible: unsafe { IsWindowVisible(hwnd) } != 0,
        owned: !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null(),
        tool_window: (unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW)
            != 0,
        title: text,
    });
    1
}

/// Snapshot all top-level HWNDs in this process. Candidate filtering and
/// collision handling happen in the portable window policy.
pub fn enumerate_owned_windows() -> Result<Vec<WindowCandidate>, SelectionError> {
    let mut candidates = Vec::new();
    let user_data = (&raw mut candidates) as isize;
    // SAFETY: callback runs synchronously and the Vec remains live and
    // exclusively owned until EnumWindows returns.
    if unsafe { EnumWindows(Some(collect_window), user_data) } == 0 {
        return Err(SelectionError::EnumerationFailed);
    }
    Ok(candidates)
}

/// Never guess which HWND belongs to the game when there are multiple viable
/// windows (e.g., game + launcher). A caller can disable speed hotkeys and
/// report a meaningful unavailable/ambiguous state instead.
pub fn find_unique_game_window() -> Result<HWND, SelectionError> {
    let id = choose_unique(&enumerate_owned_windows()?)?;
    Ok(id as usize as HWND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_never_invents_a_null_target() {
        let result = find_unique_game_window();
        if let Ok(hwnd) = result {
            assert!(!hwnd.is_null());
        }
    }
}
