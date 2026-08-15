use std::sync::{Arc, Mutex};

use intimatr::{
    command::{Command, CommandDispatcher, CommandLimits, CommandResult},
    config::{PolicyConfig, ScannerConfig},
    memory::{MemoryError, MemoryRegion, MemorySource, MemoryTarget, WritePolicy},
};

const BASE: usize = 0x4000;

#[derive(Clone)]
struct FakeMemory {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl FakeMemory {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Arc::new(Mutex::new(bytes)),
        }
    }
}

impl MemorySource for FakeMemory {
    fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
        Ok(vec![MemoryRegion {
            base: BASE,
            size: self.bytes.lock().unwrap().len(),
            committed: true,
            readable: true,
            writable: true,
            executable: true,
            guard: false,
        }])
    }

    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
        let offset = address
            .checked_sub(BASE)
            .ok_or(MemoryError::RegionNotFound {
                address,
                size: buffer.len(),
            })?;
        let end = offset
            .checked_add(buffer.len())
            .ok_or(MemoryError::AddressRangeOverflow {
                address,
                size: buffer.len(),
            })?;
        let bytes = self.bytes.lock().unwrap();
        if end > bytes.len() {
            return Err(MemoryError::RegionNotFound {
                address,
                size: buffer.len(),
            });
        }
        buffer.copy_from_slice(&bytes[offset..end]);
        Ok(())
    }
}

impl MemoryTarget for FakeMemory {
    fn write_exact(
        &self,
        address: usize,
        bytes: &[u8],
        policy: WritePolicy,
    ) -> Result<(), MemoryError> {
        if !policy.allow_memory_write {
            return Err(MemoryError::WriteDenied);
        }
        let offset = address
            .checked_sub(BASE)
            .ok_or(MemoryError::RegionNotFound {
                address,
                size: bytes.len(),
            })?;
        let end = offset
            .checked_add(bytes.len())
            .ok_or(MemoryError::AddressRangeOverflow {
                address,
                size: bytes.len(),
            })?;
        let mut target = self.bytes.lock().unwrap();
        if end > target.len() {
            return Err(MemoryError::RegionNotFound {
                address,
                size: bytes.len(),
            });
        }
        target[offset..end].copy_from_slice(bytes);
        Ok(())
    }
}

#[test]
fn disassembly_runs_through_shared_command_and_memory_layers() {
    let dispatcher = CommandDispatcher::new(
        FakeMemory::new(vec![0x48, 0x89, 0xD8, 0xC3]),
        ScannerConfig::default(),
        PolicyConfig::default(),
        CommandLimits::default(),
    );

    let result = dispatcher
        .execute(Command::Disassemble {
            address: BASE as u64,
            byte_count: 4,
            max_instructions: 8,
            bitness: 64,
        })
        .expect("disassembly should succeed")
        .result;

    match result {
        CommandResult::Disassembly { lines, .. } => {
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0].address, BASE as u64);
            assert!(lines[0].text.contains("mov"));
            assert!(lines[1].text.contains("ret"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn debugger_policy_blocks_debugger_commands_centrally() {
    let dispatcher = CommandDispatcher::new(
        FakeMemory::new(vec![0x90]),
        ScannerConfig::default(),
        PolicyConfig {
            allow_debugger: false,
            ..PolicyConfig::default()
        },
        CommandLimits::default(),
    );

    let error = dispatcher
        .execute(Command::Disassemble {
            address: BASE as u64,
            byte_count: 1,
            max_instructions: 1,
            bitness: 64,
        })
        .expect_err("debugger policy should deny disassembly");
    assert_eq!(error.code(), "policy_denied");
}

#[test]
fn disassembly_still_respects_memory_read_policy() {
    let dispatcher = CommandDispatcher::new(
        FakeMemory::new(vec![0x90]),
        ScannerConfig::default(),
        PolicyConfig {
            allow_memory_read: false,
            ..PolicyConfig::default()
        },
        CommandLimits::default(),
    );

    let error = dispatcher
        .execute(Command::Disassemble {
            address: BASE as u64,
            byte_count: 1,
            max_instructions: 1,
            bitness: 64,
        })
        .expect_err("memory read policy should deny disassembly");
    assert_eq!(error.code(), "policy_denied");
}
