from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing marker: {label}")
    return text.replace(old, new, 1)


path = Path("src/command.rs")
text = path.read_text()
text = replace_once(
    text,
    "        atomic::{AtomicU64, Ordering},\n",
    "        atomic::{AtomicBool, AtomicU64, Ordering},\n",
    "AtomicBool import",
)
text = replace_once(
    text,
    "    next_scan_id: AtomicU64,\n    next_watch_id: AtomicU64,\n",
    "    next_scan_id: AtomicU64,\n    next_watch_id: AtomicU64,\n    shutting_down: AtomicBool,\n",
    "dispatcher shutdown field",
)
text = replace_once(
    text,
    "            next_scan_id: AtomicU64::new(1),\n            next_watch_id: AtomicU64::new(1),\n",
    "            next_scan_id: AtomicU64::new(1),\n            next_watch_id: AtomicU64::new(1),\n            shutting_down: AtomicBool::new(false),\n",
    "dispatcher shutdown initialization",
)
text = replace_once(
    text,
    "    pub fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {\n        let command_name = command.name();\n",
    "    pub fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {\n        if self.shutting_down.load(Ordering::Acquire) {\n            return Err(CommandError::ShuttingDown);\n        }\n        let command_name = command.name();\n",
    "dispatcher shutdown gate",
)
text = replace_once(
    text,
    "    fn shutdown(&self) {\n        if let Err(error) = self.cancel_all_scans() {\n",
    "    fn shutdown(&self) {\n        self.shutting_down.store(true, Ordering::Release);\n        if let Err(error) = self.cancel_all_scans() {\n",
    "dispatcher shutdown flag",
)
text = replace_once(
    text,
    "    #[error(\"shared command state mutex was poisoned\")]\n    StatePoisoned,\n",
    "    #[error(\"shared command state mutex was poisoned\")]\n    StatePoisoned,\n    #[error(\"shared command executor is shutting down\")]\n    ShuttingDown,\n",
    "shutdown command error",
)
text = replace_once(
    text,
    "            Self::StatePoisoned => \"state_poisoned\",\n",
    "            Self::StatePoisoned => \"state_poisoned\",\n            Self::ShuttingDown => \"shutting_down\",\n",
    "shutdown error code",
)
path.write_text(text)

Path("tests/shutdown_gate.rs").write_text('''use intimatr::{
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
''')
