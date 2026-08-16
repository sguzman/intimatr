use intimatr::{
    command::{Command, CommandDispatcher, CommandExecutor, CommandLimits},
    config::{PolicyConfig, ScannerConfig},
    memory::{MemoryError, MemoryRegion, MemorySource, MemoryTarget, WritePolicy},
};

struct EmptyMemory;

impl MemorySource for EmptyMemory {
    fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
        Ok(Vec::new())
    }

    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::RegionNotFound {
            address,
            size: buffer.len(),
        })
    }
}

impl MemoryTarget for EmptyMemory {
    fn write_exact(
        &self,
        address: usize,
        bytes: &[u8],
        _policy: WritePolicy,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::RegionNotFound {
            address,
            size: bytes.len(),
        })
    }
}

#[test]
fn dispatcher_rejects_new_commands_after_shutdown_begins() {
    let dispatcher = CommandDispatcher::new(
        EmptyMemory,
        ScannerConfig::default(),
        PolicyConfig::default(),
        CommandLimits::default(),
    );

    CommandExecutor::shutdown(&dispatcher);
    let error = dispatcher
        .execute(Command::Ping)
        .expect_err("new command work must be rejected after shutdown starts");
    assert_eq!(error.code(), "shutting_down");
}
