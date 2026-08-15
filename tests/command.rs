use std::sync::{Arc, Mutex};

use intimatr::{
    command::{Command, CommandDispatcher, CommandLimits, CommandResult},
    config::{PolicyConfig, ScannerConfig},
    memory::{MemoryError, MemoryRegion, MemorySource, MemoryTarget, WritePolicy},
    scanner::{ScalarValue, ScanPredicate, ValueType},
};

const BASE: usize = 0x1000;

#[derive(Clone)]
struct FakeMemory {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl FakeMemory {
    fn from_i32(values: &[i32]) -> Self {
        let bytes = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        Self {
            bytes: Arc::new(Mutex::new(bytes)),
        }
    }

    fn range(&self, address: usize, size: usize) -> Result<std::ops::Range<usize>, MemoryError> {
        let offset = address
            .checked_sub(BASE)
            .ok_or(MemoryError::RegionNotFound { address, size })?;
        let end = offset
            .checked_add(size)
            .ok_or(MemoryError::AddressRangeOverflow { address, size })?;
        let len = self.bytes.lock().unwrap().len();
        if end > len {
            return Err(MemoryError::RegionNotFound { address, size });
        }
        Ok(offset..end)
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
            executable: false,
            guard: false,
        }])
    }

    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
        let range = self.range(address, buffer.len())?;
        buffer.copy_from_slice(&self.bytes.lock().unwrap()[range]);
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
        let range = self.range(address, bytes.len())?;
        self.bytes.lock().unwrap()[range].copy_from_slice(bytes);
        Ok(())
    }
}

fn dispatcher(memory: FakeMemory) -> CommandDispatcher<FakeMemory> {
    let scanner = ScannerConfig {
        alignment: 4,
        chunk_size_bytes: 8,
        max_results: 128,
        ..ScannerConfig::default()
    };
    CommandDispatcher::new(
        memory,
        scanner,
        PolicyConfig::default(),
        CommandLimits {
            max_memory_transfer_bytes: 1024,
            max_scan_results_per_page: 32,
        },
    )
}

#[test]
fn policy_gates_memory_reads_in_shared_command_layer() {
    let memory = FakeMemory::from_i32(&[10]);
    let policy = PolicyConfig {
        allow_memory_read: false,
        ..PolicyConfig::default()
    };
    let dispatcher = CommandDispatcher::new(
        memory,
        ScannerConfig::default(),
        policy,
        CommandLimits::default(),
    );

    let error = dispatcher
        .execute(Command::ReadMemory {
            address: BASE as u64,
            size: 4,
        })
        .expect_err("read should be policy gated");

    assert_eq!(error.code(), "policy_denied");
}

#[test]
fn raw_and_typed_memory_commands_share_the_memory_backend() {
    let memory = FakeMemory::from_i32(&[10, 20]);
    let dispatcher = dispatcher(memory);

    dispatcher
        .execute(Command::WriteScalar {
            address: (BASE + 4) as u64,
            value_type: ValueType::I32,
            value: ScalarValue::Signed(25),
        })
        .expect("typed write should succeed");

    let result = dispatcher
        .execute(Command::ReadScalar {
            address: (BASE + 4) as u64,
            value_type: ValueType::I32,
        })
        .expect("typed read should succeed")
        .result;

    assert_eq!(
        result,
        CommandResult::Scalar {
            address: (BASE + 4) as u64,
            value_type: ValueType::I32,
            value: ScalarValue::Signed(25),
        }
    );
}

#[test]
fn scan_sessions_support_unknown_initial_and_historical_refinement() {
    let memory = FakeMemory::from_i32(&[10, 20, 30]);
    let dispatcher = dispatcher(memory.clone());

    let first = dispatcher
        .execute(Command::FirstScan {
            value_type: ValueType::I32,
            predicate: ScanPredicate::UnknownInitialValue,
        })
        .expect("unknown initial scan should succeed")
        .result;

    let scan_id = match first {
        CommandResult::Scan { summary } => {
            assert_eq!(summary.result_count, 3);
            summary.scan_id
        }
        other => panic!("unexpected result: {other:?}"),
    };

    memory
        .write_exact(
            BASE + 4,
            &25_i32.to_le_bytes(),
            WritePolicy {
                allow_memory_write: true,
                allow_code_patch: false,
            },
        )
        .unwrap();

    let refined = dispatcher
        .execute(Command::NextScan {
            scan_id,
            predicate: ScanPredicate::Increased,
        })
        .expect("next scan should succeed")
        .result;

    match refined {
        CommandResult::Scan { summary } => assert_eq!(summary.result_count, 1),
        other => panic!("unexpected result: {other:?}"),
    }

    let page = dispatcher
        .execute(Command::ScanResults {
            scan_id,
            offset: 0,
            limit: 8,
        })
        .expect("result page should succeed")
        .result;

    match page {
        CommandResult::ScanResults { candidates, .. } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].address, (BASE + 4) as u64);
            assert_eq!(candidates[0].current, ScalarValue::Signed(25));
            assert_eq!(candidates[0].previous, Some(ScalarValue::Signed(20)));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn watches_are_shared_frontend_state_and_refresh_through_memory_core() {
    let memory = FakeMemory::from_i32(&[42]);
    let dispatcher = dispatcher(memory);

    let added = dispatcher
        .execute(Command::AddWatch {
            address: BASE as u64,
            value_type: ValueType::I32,
            label: Some("health".to_owned()),
        })
        .expect("watch should be added")
        .result;

    let watch_id = match added {
        CommandResult::WatchAdded { watch } => watch.id,
        other => panic!("unexpected result: {other:?}"),
    };

    let refreshed = dispatcher
        .execute(Command::RefreshWatches)
        .expect("watch refresh should succeed")
        .result;

    match refreshed {
        CommandResult::WatchValues { values } => {
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].watch.id, watch_id);
            assert_eq!(values[0].value, Some(ScalarValue::Signed(42)));
            assert!(values[0].error.is_none());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn paging_and_transfer_limits_are_enforced_centrally() {
    let memory = FakeMemory::from_i32(&[1, 2, 3]);
    let dispatcher = CommandDispatcher::new(
        memory,
        ScannerConfig::default(),
        PolicyConfig::default(),
        CommandLimits {
            max_memory_transfer_bytes: 2,
            max_scan_results_per_page: 1,
        },
    );

    let error = dispatcher
        .execute(Command::ReadMemory {
            address: BASE as u64,
            size: 4,
        })
        .expect_err("oversized read should fail");
    assert_eq!(error.code(), "limit_exceeded");
}
