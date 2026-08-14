use thiserror::Error;
use tracing::trace;

use crate::{
    config::{PolicyConfig, ScannerConfig},
    scanner::{ScalarValue, ValueError, ValueType},
};

#[cfg(windows)]
pub use crate::platform::windows::memory::CurrentProcessMemory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
    pub committed: bool,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub guard: bool,
}

impl MemoryRegion {
    pub fn end(self) -> Result<usize, MemoryError> {
        self.base
            .checked_add(self.size)
            .ok_or(MemoryError::AddressRangeOverflow {
                address: self.base,
                size: self.size,
            })
    }

    pub fn contains_range(self, address: usize, size: usize) -> bool {
        let Some(region_end) = self.base.checked_add(self.size) else {
            return false;
        };
        let Some(range_end) = address.checked_add(size) else {
            return false;
        };

        address >= self.base && range_end <= region_end
    }

    pub fn is_scannable(self, filter: RegionFilter) -> bool {
        self.committed
            && (!filter.require_readable || self.readable)
            && (!filter.require_writable || self.writable)
            && (!filter.require_executable || self.executable)
            && (filter.include_guard_pages || !self.guard)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionFilter {
    pub require_readable: bool,
    pub require_writable: bool,
    pub require_executable: bool,
    pub include_guard_pages: bool,
}

impl From<&ScannerConfig> for RegionFilter {
    fn from(config: &ScannerConfig) -> Self {
        Self {
            require_readable: config.require_readable,
            require_writable: config.require_writable,
            require_executable: config.require_executable,
            include_guard_pages: config.include_guard_pages,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WritePolicy {
    pub allow_memory_write: bool,
    pub allow_code_patch: bool,
}

impl From<&PolicyConfig> for WritePolicy {
    fn from(config: &PolicyConfig) -> Self {
        Self {
            allow_memory_write: config.allow_memory_write,
            allow_code_patch: config.allow_code_patch,
        }
    }
}

pub trait MemorySource {
    fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError>;
    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError>;
}

pub trait MemoryTarget: MemorySource {
    fn write_exact(
        &self,
        address: usize,
        bytes: &[u8],
        policy: WritePolicy,
    ) -> Result<(), MemoryError>;
}

pub fn read_scalar<S: MemorySource + ?Sized>(
    source: &S,
    address: usize,
    value_type: ValueType,
) -> Result<ScalarValue, MemoryError> {
    let mut bytes = [0_u8; 8];
    let width = value_type.byte_width();
    source.read_exact(address, &mut bytes[..width])?;
    let value = value_type.decode(&bytes[..width])?;
    trace!(address, ?value_type, ?value, "read typed memory value");
    Ok(value)
}

pub fn write_scalar<T: MemoryTarget + ?Sized>(
    target: &T,
    address: usize,
    value_type: ValueType,
    value: ScalarValue,
    policy: WritePolicy,
) -> Result<(), MemoryError> {
    let bytes = value_type.encode(value)?;
    target.write_exact(address, &bytes, policy)?;
    trace!(address, ?value_type, ?value, "wrote typed memory value");
    Ok(())
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("Windows/platform memory operation {operation} failed with error code {code}")]
    Platform { operation: &'static str, code: u32 },
    #[error(
        "memory operation at 0x{address:X} requested {requested} bytes but transferred {actual}"
    )]
    PartialTransfer {
        address: usize,
        requested: usize,
        actual: usize,
    },
    #[error("address range overflow at 0x{address:X} + {size} bytes")]
    AddressRangeOverflow { address: usize, size: usize },
    #[error("no committed memory region contains 0x{address:X} + {size} bytes")]
    RegionNotFound { address: usize, size: usize },
    #[error("memory writes are disabled by policy")]
    WriteDenied,
    #[error("writing executable memory is disabled by the code-patch policy")]
    CodePatchDenied,
    #[error("refusing to write a guard-page region")]
    GuardPageDenied,
    #[error("failed to restore memory protection after a write; Windows error code {code}")]
    ProtectionRestoreFailed { code: u32 },
    #[error(transparent)]
    Value(#[from] ValueError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> MemoryRegion {
        MemoryRegion {
            base: 0x1000,
            size: 0x1000,
            committed: true,
            readable: true,
            writable: false,
            executable: false,
            guard: false,
        }
    }

    #[test]
    fn region_filter_honors_access_requirements() {
        let filter = RegionFilter {
            require_readable: true,
            require_writable: false,
            require_executable: false,
            include_guard_pages: false,
        };
        assert!(region().is_scannable(filter));

        let writable_only = RegionFilter {
            require_writable: true,
            ..filter
        };
        assert!(!region().is_scannable(writable_only));
    }

    #[test]
    fn guard_pages_are_excluded_by_default() {
        let guarded = MemoryRegion {
            guard: true,
            ..region()
        };
        let filter = RegionFilter {
            require_readable: true,
            require_writable: false,
            require_executable: false,
            include_guard_pages: false,
        };

        assert!(!guarded.is_scannable(filter));
        assert!(guarded.is_scannable(RegionFilter {
            include_guard_pages: true,
            ..filter
        }));
    }
}
