//! Windows-only reader for one explicit in-process game HWND.
//!
//! Deliberately not a global keyboard hook. Neither PID-only matching nor
//! "some foreground window" permits key sampling. The host must supply a
//! real, owned game HWND and choose when to construct/poll this reader.

use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::GetCurrentProcessId,
    UI::{
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, IsWindow},
    },
};

use crate::speed_input::InputSnapshot;

pub struct WindowsFocusedInput {
    target_window: HWND,
    watched_keys: Vec<u16>,
}

impl WindowsFocusedInput {
    /// Reject null, destroyed, and foreign-process handles before the first
    /// read. Matching only an executable basename does not establish HWND
    /// ownership.
    pub fn new(target_window: HWND, watched_keys: &[u16]) -> Result<Self, &'static str> {
        if !Self::is_owned_window(target_window) {
            return Err("target game HWND is invalid or belongs to another process");
        }
        let mut keys = watched_keys.to_vec();
        keys.sort_unstable();
        keys.dedup();
        Ok(Self {
            target_window,
            watched_keys: keys,
        })
    }

    fn is_owned_window(window: HWND) -> bool {
        if window.is_null() || unsafe { IsWindow(window) } == 0 {
            return false;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        pid != 0 && pid == unsafe { GetCurrentProcessId() }
    }

    /// A different foreground HWND (including Intimatr's own tool windows)
    /// yields unfocused without observing any keyboard state.
    pub fn sample(&self) -> InputSnapshot {
        if !Self::is_owned_window(self.target_window)
            || unsafe { GetForegroundWindow() } != self.target_window
        {
            return InputSnapshot::default();
        }
        let down_keys = self
            .watched_keys
            .iter()
            .copied()
            .filter(|key| unsafe { GetAsyncKeyState(i32::from(*key)) } as u16 & 0x8000 != 0)
            .collect();
        InputSnapshot {
            focused: true,
            down_keys,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsFocusedInput;

    #[test]
    fn refuses_null_target_window() {
        assert!(WindowsFocusedInput::new(std::ptr::null_mut(), &[0x11, 0x76]).is_err());
    }
}
