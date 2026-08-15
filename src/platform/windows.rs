use std::{
    ffi::{OsStr, OsString},
    mem::size_of,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HMODULE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::OutputDebugStringW,
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPTHREAD, THREADENTRY32,
                Thread32First, Thread32Next,
            },
        },
        LibraryLoader::GetModuleFileNameW,
        Threading::GetCurrentProcessId,
    },
};

pub mod memory;

const INITIAL_PATH_CAPACITY: usize = 260;
const MAX_PATH_CAPACITY: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModule {
    pub name: String,
    pub path: String,
    pub base: u64,
    pub size: u64,
}

pub fn current_process_executable() -> Result<PathBuf, WindowsError> {
    module_file_name(std::ptr::null_mut())
}

pub fn loaded_module_path(module: HMODULE) -> Result<PathBuf, WindowsError> {
    module_file_name(module)
}

pub fn loaded_modules() -> Result<Vec<LoadedModule>, WindowsError> {
    let process_id = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id)
    };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_api_error("CreateToolhelp32Snapshot(modules)"));
    }
    let _snapshot = SnapshotHandle(snapshot);

    let mut entry = MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Module32FirstW(snapshot, &mut entry) } == 0 {
        return Err(last_api_error("Module32FirstW"));
    }

    let mut modules = Vec::new();
    loop {
        modules.push(LoadedModule {
            name: wide_buffer_to_string(&entry.szModule),
            path: wide_buffer_to_string(&entry.szExePath),
            base: entry.modBaseAddr as usize as u64,
            size: entry.modBaseSize as u64,
        });

        entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
        if unsafe { Module32NextW(snapshot, &mut entry) } == 0 {
            break;
        }
    }

    modules.sort_unstable_by_key(|module| module.base);
    Ok(modules)
}

pub fn current_process_threads() -> Result<Vec<u32>, WindowsError> {
    let process_id = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_api_error("CreateToolhelp32Snapshot(threads)"));
    }
    let _snapshot = SnapshotHandle(snapshot);

    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    if unsafe { Thread32First(snapshot, &mut entry) } == 0 {
        return Err(last_api_error("Thread32First"));
    }

    let mut threads = Vec::new();
    loop {
        if entry.th32OwnerProcessID == process_id {
            threads.push(entry.th32ThreadID);
        }

        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        if unsafe { Thread32Next(snapshot, &mut entry) } == 0 {
            break;
        }
    }

    threads.sort_unstable();
    Ok(threads)
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
            return Err(last_api_error("GetModuleFileNameW"));
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

fn wide_buffer_to_string(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..length])
        .to_string_lossy()
        .into_owned()
}

fn last_api_error(operation: &'static str) -> WindowsError {
    WindowsError::Api {
        operation,
        code: unsafe { GetLastError() },
    }
}

struct SnapshotHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for SnapshotHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
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
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    #[test]
    fn resolves_current_process_executable() {
        let path = current_process_executable().expect("current process path should resolve");

        assert!(path.is_absolute());
        assert!(path.file_name().is_some());
    }

    #[test]
    fn enumerates_loaded_modules() {
        let modules = loaded_modules().expect("current process modules should enumerate");

        assert!(!modules.is_empty());
        assert!(modules.iter().all(|module| !module.name.is_empty()));
    }

    #[test]
    fn enumerates_current_process_threads() {
        let current_thread = unsafe { GetCurrentThreadId() };
        let threads = current_process_threads().expect("current process threads should enumerate");

        assert!(threads.contains(&current_thread));
    }
}
