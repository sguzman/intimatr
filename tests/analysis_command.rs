use std::{
    fs,
    sync::{Arc, Mutex},
};

use intimatr::{
    analysis::{
        AnalysisCommand, AnalysisResult, PointerChainSpec, StructureFieldKind, StructureFieldSpec,
    },
    command::{Command, CommandDispatcher, CommandLimits, CommandResult},
    config::{PolicyConfig, ScannerConfig},
    memory::{MemoryError, MemoryRegion, MemorySource, MemoryTarget, WritePolicy},
    scanner::{ScalarValue, ScanPredicate, ValueType},
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
            executable: false,
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

fn dispatcher(bytes: Vec<u8>) -> CommandDispatcher<FakeMemory> {
    let directory = std::env::temp_dir().join(format!(
        "intimatr-analysis-command-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&directory);
    CommandDispatcher::new(
        FakeMemory::new(bytes),
        ScannerConfig::default(),
        PolicyConfig::default(),
        CommandLimits::default(),
    )
    .with_analysis_directory(directory)
}

fn analysis(
    dispatcher: &CommandDispatcher<FakeMemory>,
    request: AnalysisCommand,
) -> AnalysisResult {
    match dispatcher
        .execute(Command::Analysis { request })
        .expect("analysis command should succeed")
        .result
    {
        CommandResult::Analysis { analysis } => analysis,
        other => panic!("unexpected command result: {other:?}"),
    }
}

#[test]
fn aob_and_structure_analysis_share_the_command_layer() {
    let mut bytes = vec![0_u8; 64];
    bytes[4..8].copy_from_slice(&[0x48, 0x8B, 0xAA, 0xFF]);
    bytes[16..20].copy_from_slice(&1234_u32.to_le_bytes());
    let dispatcher = dispatcher(bytes);

    match analysis(
        &dispatcher,
        AnalysisCommand::AobScan {
            pattern: "48 8B ?? FF".to_owned(),
            alignment: 1,
            max_results: 32,
        },
    ) {
        AnalysisResult::PatternScan { scan } => assert_eq!(scan.addresses, vec![(BASE + 4) as u64]),
        other => panic!("unexpected analysis result: {other:?}"),
    }

    match analysis(
        &dispatcher,
        AnalysisCommand::InspectStructure {
            base: format!("0x{:X}", BASE),
            fields: vec![StructureFieldSpec {
                name: "health".to_owned(),
                offset: 16,
                kind: StructureFieldKind::Scalar {
                    value_type: ValueType::U32,
                },
            }],
        },
    ) {
        AnalysisResult::Structure { fields } => {
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].address, (BASE + 16) as u64);
        }
        other => panic!("unexpected analysis result: {other:?}"),
    }
}

#[test]
fn saved_scans_and_watch_templates_round_trip_through_workspace() {
    let mut bytes = vec![0_u8; 64];
    bytes[8..12].copy_from_slice(&77_u32.to_le_bytes());
    let dispatcher = dispatcher(bytes);

    let scan_id = match dispatcher
        .execute(Command::FirstScan {
            value_type: ValueType::U32,
            predicate: ScanPredicate::Exact(ScalarValue::Unsigned(77)),
        })
        .unwrap()
        .result
    {
        CommandResult::Scan { summary } => summary.scan_id,
        other => panic!("unexpected scan result: {other:?}"),
    };

    analysis(
        &dispatcher,
        AnalysisCommand::SaveScan {
            scan_id,
            name: "health_scan".to_owned(),
        },
    );
    let watch_id = match dispatcher
        .execute(Command::AddWatch {
            address: (BASE + 8) as u64,
            value_type: ValueType::U32,
            label: Some("Health".to_owned()),
        })
        .unwrap()
        .result
    {
        CommandResult::WatchAdded { watch } => watch.id,
        other => panic!("unexpected watch result: {other:?}"),
    };
    analysis(
        &dispatcher,
        AnalysisCommand::SaveWatchTemplate {
            watch_id,
            name: "health_watch".to_owned(),
        },
    );
    analysis(
        &dispatcher,
        AnalysisCommand::SaveWorkspace {
            name: "profile".to_owned(),
        },
    );

    match analysis(&dispatcher, AnalysisCommand::ListSaved) {
        AnalysisResult::Saved { summary } => {
            assert_eq!(summary.scans, vec!["health_scan"]);
            assert_eq!(summary.watch_templates, vec!["health_watch"]);
        }
        other => panic!("unexpected list result: {other:?}"),
    }

    match analysis(
        &dispatcher,
        AnalysisCommand::RestoreScan {
            name: "health_scan".to_owned(),
        },
    ) {
        AnalysisResult::ScanRestored {
            scan_id: restored, ..
        } => assert_ne!(restored, scan_id),
        other => panic!("unexpected restore result: {other:?}"),
    }
    match analysis(
        &dispatcher,
        AnalysisCommand::AddWatchFromTemplate {
            name: "health_watch".to_owned(),
            label: None,
        },
    ) {
        AnalysisResult::WatchAdded {
            watch_id: restored, ..
        } => assert_ne!(restored, watch_id),
        other => panic!("unexpected watch-template result: {other:?}"),
    }
}

#[test]
fn pointer_chain_and_batch_are_rpc_serializable_analysis_primitives() {
    let mut bytes = vec![0_u8; 64];
    bytes[0..8].copy_from_slice(&((BASE + 24) as u64).to_le_bytes());
    let dispatcher = dispatcher(bytes);

    match analysis(
        &dispatcher,
        AnalysisCommand::Batch {
            commands: vec![
                AnalysisCommand::ResolveAddress {
                    expression: format!("0x{:X}", BASE + 10),
                },
                AnalysisCommand::ResolvePointerChain {
                    spec: PointerChainSpec {
                        base: format!("0x{:X}", BASE),
                        offsets: vec![4],
                        pointer_size: 8,
                    },
                },
            ],
        },
    ) {
        AnalysisResult::Batch { results } => {
            assert_eq!(results.len(), 2);
            assert!(matches!(results[0], AnalysisResult::Address { .. }));
            assert!(matches!(results[1], AnalysisResult::PointerChain { .. }));
        }
        other => panic!("unexpected batch result: {other:?}"),
    }
}
