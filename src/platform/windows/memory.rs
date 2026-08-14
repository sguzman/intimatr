use std::{ffi::c_void, mem};

use tracing::{debug, trace};
use windows_sys::Win32::{
    Foundation::GetLastError,
    System::{
        Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
        Memory::{
            MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
            PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY,
            PAGE_READWRITE, PAGE_WRITECOPY, VirtualProtect, VirtualQuery,
        },
        Threading::{FlushInstructionCache, GetCurrentProcess},
    },
};

use crate::memory::{MemoryError, MemoryRegion, MemorySource, MemoryTarget, WritePolicy};

const BASE_PROTECTION_MASK: u32 = 0xff;

#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentProcessMemory;

impl CurrentProcessMemory {
    pub const fn new() -> Self {
        Self
    }

    fn query_region(&self, address: usize) -> Result<MemoryRegion, MemoryError> {
        let mut information: MEMORY_BASIC_INFORMATION = unsafe { mem::zeroed() };
        let queried = unsafe {
            VirtualQuery(
                address as *const c_void,
                &mut information,
                mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == 0 {
            return Err(platform_error("VirtualQuery"));
        }

        Ok(normalize_region(&information))
    }

    fn write_raw(&self, address: usize, bytes: &[u8]) -> Result<(), MemoryError> {
        if bytes.is_empty() {
            return Ok(());
        }

        let mut written = 0_usize;
        let succeeded = unsafe {
            WriteProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut written,
            )
        };
        if succeeded == 0 {
            return Err(platform_error("WriteProcessMemory"));
        }
        if written != bytes.len() {
            return Err(MemoryError::PartialTransfer {
                address,
                requested: bytes.len(),
                actual: written,
            });
        }

        Ok(())
    }
}

impl MemorySource for CurrentProcessMemory {
    fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
        let mut regions = Vec::new();
        let mut address = 0_usize;

        loop {
            let mut information: MEMORY_BASIC_INFORMATION = unsafe { mem::zeroed() };
            let queried = unsafe {
                VirtualQuery(
                    address as *const c_void,
                    &mut information,
                    mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if queried == 0 {
                if regions.is_empty() {
                    return Err(platform_error("VirtualQuery"));
                }
                break;
            }

            let region = normalize_region(&information);
            if region.size == 0 {
                break;
            }
            let next = region.end()?;
            if next <= address {
                break;
            }

            trace!(
                base = region.base,
                size = region.size,
                committed = region.committed,
                readable = region.readable,
                writable = region.writable,
                executable = region.executable,
                guard = region.guard,
                "enumerated process memory region"
            );
            regions.push(region);
            address = next;
        }

        debug!(
            region_count = regions.len(),
            "enumerated current process memory map"
        );
        Ok(regions)
    }

    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
        if buffer.is_empty() {
            return Ok(());
        }

        let mut read = 0_usize;
        let succeeded = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut read,
            )
        };
        if succeeded == 0 {
            return Err(platform_error("ReadProcessMemory"));
        }
        if read != buffer.len() {
            return Err(MemoryError::PartialTransfer {
                address,
                requested: buffer.len(),
                actual: read,
            });
        }

        Ok(())
    }
}

impl MemoryTarget for CurrentProcessMemory {
    fn write_exact(
        &self,
        address: usize,
        bytes: &[u8],
        policy: WritePolicy,
    ) -> Result<(), MemoryError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if !policy.allow_memory_write {
            return Err(MemoryError::WriteDenied);
        }

        let region = self.query_region(address)?;
        if !region.committed || !region.contains_range(address, bytes.len()) {
            return Err(MemoryError::RegionNotFound {
                address,
                size: bytes.len(),
            });
        }
        if region.guard {
            return Err(MemoryError::GuardPageDenied);
        }
        if region.executable && !policy.allow_code_patch {
            return Err(MemoryError::CodePatchDenied);
        }

        let changed_protection = !region.writable;
        let mut old_protection = 0_u32;
        if changed_protection {
            let temporary_protection = if region.executable {
                PAGE_EXECUTE_READWRITE
            } else {
                PAGE_READWRITE
            };
            let succeeded = unsafe {
                VirtualProtect(
                    address as *const c_void,
                    bytes.len(),
                    temporary_protection,
                    &mut old_protection,
                )
            };
            if succeeded == 0 {
                return Err(platform_error("VirtualProtect"));
            }
        }

        let write_result = self.write_raw(address, bytes);

        if changed_protection {
            let mut ignored = 0_u32;
            let restored = unsafe {
                VirtualProtect(
                    address as *const c_void,
                    bytes.len(),
                    old_protection,
                    &mut ignored,
                )
            };
            if restored == 0 {
                return Err(MemoryError::ProtectionRestoreFailed {
                    code: unsafe { GetLastError() },
                });
            }
        }

        write_result?;

        if region.executable {
            let flushed = unsafe {
                FlushInstructionCache(GetCurrentProcess(), address as *const c_void, bytes.len())
            };
            if flushed == 0 {
                return Err(platform_error("FlushInstructionCache"));
            }
        }

        trace!(
            address,
            size = bytes.len(),
            changed_protection,
            executable = region.executable,
            "completed current-process memory write"
        );
        Ok(())
    }
}

fn normalize_region(information: &MEMORY_BASIC_INFORMATION) -> MemoryRegion {
    let protection = information.Protect;
    let base_protection = protection & BASE_PROTECTION_MASK;

    MemoryRegion {
        base: information.BaseAddress as usize,
        size: information.RegionSize,
        committed: information.State == MEM_COMMIT,
        readable: matches!(
            base_protection,
            PAGE_READONLY
                | PAGE_READWRITE
                | PAGE_WRITECOPY
                | PAGE_EXECUTE_READ
                | PAGE_EXECUTE_READWRITE
                | PAGE_EXECUTE_WRITECOPY
        ),
        writable: matches!(
            base_protection,
            PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        ),
        executable: matches!(
            base_protection,
            PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        ),
        guard: protection & PAGE_GUARD != 0,
    }
}

fn platform_error(operation: &'static str) -> MemoryError {
    MemoryError::Platform {
        operation,
        code: unsafe { GetLastError() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_backend_reads_stack_memory() {
        let value = 0x1234_5678_u32;
        let address = std::ptr::addr_of!(value) as usize;
        let mut bytes = [0_u8; 4];

        CurrentProcessMemory::new()
            .read_exact(address, &mut bytes)
            .expect("stack value should be readable");

        assert_eq!(u32::from_le_bytes(bytes), value);
    }

    #[test]
    fn current_process_map_contains_stack_value() {
        let value = 17_u64;
        let address = std::ptr::addr_of!(value) as usize;
        let regions = CurrentProcessMemory::new()
            .regions()
            .expect("process memory map should enumerate");

        assert!(
            regions
                .iter()
                .any(|region| region.contains_range(address, 8))
        );
    }
}
