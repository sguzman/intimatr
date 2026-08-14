use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{GetLastError, HMODULE},
    System::{Diagnostics::Debug::OutputDebugStringW, LibraryLoader::GetModuleFileNameW},
};

pub mod memory;

const INITIAL_PATH_CAPACITY: usize = 260;
const MAX_PATH_CAPACITY: usize = 32_768;

pub fn current_process_executable() -> Result<PathBuf, WindowsError> {
    module_file_name(std::ptr::null_mut())
}

pub fn loaded_module_path(module: HMODULE) -> Result<PathBuf, WindowsError> {
    module_file_name(module)
}

pub fn debug_output(message: &str) {
    let wide: Vec<u16> = OsStr::new(message).encode_wide().chain(Some(0)).collect();
    unsafe {
        OutputDebugStringW(wide.as_ptr());
    }
}

fn module_file_name(module: HMODULE) -> Result<PathBuf, WindowsError> {
    let mut capacity = INITIAL_PATH_CAPACITY;

    loop {
        let mut buffer = vec![0_u16; capacity];
        let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), capacity as u32) };

        if length == 0 {
            return Err(WindowsError::Api {
                operation: "GetModuleFileNameW",
                code: unsafe { GetLastError() },
            });
        }

        if (length as usize) < capacity {
            buffer.truncate(length as usize);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }

        if capacity >= MAX_PATH_CAPACITY {
            return Err(WindowsError::PathTooLong {
                limit: MAX_PATH_CAPACITY,
            });
        }

        capacity = (capacity * 2).min(MAX_PATH_CAPACITY);
    }
}

#[derive(Debug, Error)]
pub enum WindowsError {
    #[error("Windows API call {operation} failed with error code {code}")]
    Api { operation: &'static str, code: u32 },
    #[error("module path exceeded the supported {limit}-character buffer")]
    PathTooLong { limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_current_process_executable() {
        let path = current_process_executable().expect("current process path should resolve");

        assert!(path.is_absolute());
        assert!(path.file_name().is_some());
    }
}
