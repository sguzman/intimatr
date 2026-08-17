use intimatr::{
    memory::{MemoryError, MemoryRegion, MemorySource},
    scanner::{
        CancellationToken, RegionFilterConfig, ScalarValue, ScanError, ScanOptions, ScanPredicate,
        ValueType, first_scan, first_scan_with_progress,
    },
};

const BASE: usize = 0x1000;

#[derive(Clone)]
struct SyntheticMemory {
    bytes: Vec<u8>,
}

impl SyntheticMemory {
    fn from_i32(values: &[i32]) -> Self {
        let bytes = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        Self { bytes }
    }
}

impl MemorySource for SyntheticMemory {
    fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
        Ok(vec![MemoryRegion {
            base: BASE,
            size: self.bytes.len(),
            committed: true,
            readable: true,
            writable: true,
            executable: false,
            guard: false,
        }])
    }

    fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
        let Some(offset) = address.checked_sub(BASE) else {
            return Err(MemoryError::RegionNotFound {
                address,
                size: buffer.len(),
            });
        };
        let Some(end) = offset.checked_add(buffer.len()) else {
            return Err(MemoryError::AddressRangeOverflow {
                address,
                size: buffer.len(),
            });
        };
        if end > self.bytes.len() {
            return Err(MemoryError::RegionNotFound {
                address,
                size: buffer.len(),
            });
        }

        buffer.copy_from_slice(&self.bytes[offset..end]);
        Ok(())
    }
}

fn options() -> ScanOptions {
    ScanOptions {
        chunk_size_bytes: 7,
        worker_threads: 4,
        alignment: 4,
        max_results: 100,
        float_epsilon: 1.0e-6,
        region_filter: RegionFilterConfig {
            require_readable: true,
            require_writable: false,
            require_executable: false,
            include_guard_pages: false,
        },
    }
}

#[test]
fn first_scan_finds_exact_values_across_chunk_boundaries() {
    let memory = SyntheticMemory::from_i32(&[1, 100, 2, 3, 4, 100, 5]);
    let session = first_scan(
        &memory,
        ValueType::I32,
        ScanPredicate::Exact(ScalarValue::Signed(100)),
        options(),
        &CancellationToken::new(),
    )
    .expect("scan should succeed");

    let addresses: Vec<_> = session
        .candidates
        .iter()
        .map(|result| result.address)
        .collect();
    assert_eq!(addresses, vec![BASE + 4, BASE + 20]);
    assert_eq!(session.stats.read_failures, 0);
}

#[test]
fn unknown_initial_value_can_be_refined_by_changed_and_decreased() {
    let initial = SyntheticMemory::from_i32(&[10, 20, 30]);
    let initial_session = first_scan(
        &initial,
        ValueType::I32,
        ScanPredicate::UnknownInitialValue,
        options(),
        &CancellationToken::new(),
    )
    .expect("unknown initial scan should succeed");
    assert_eq!(initial_session.candidates.len(), 3);

    let updated = SyntheticMemory::from_i32(&[10, 15, 35]);
    let changed = initial_session
        .next_scan(
            &updated,
            ScanPredicate::Changed,
            options(),
            &CancellationToken::new(),
        )
        .expect("changed scan should succeed");
    assert_eq!(changed.candidates.len(), 2);
    assert_eq!(
        changed.candidates[0].previous,
        Some(ScalarValue::Signed(20))
    );
    assert_eq!(changed.candidates[0].current, ScalarValue::Signed(15));

    let decreased = initial_session
        .next_scan(
            &updated,
            ScanPredicate::Decreased,
            options(),
            &CancellationToken::new(),
        )
        .expect("decreased scan should succeed");
    assert_eq!(decreased.candidates.len(), 1);
    assert_eq!(decreased.candidates[0].address, BASE + 4);
}

#[test]
fn parallel_and_serial_first_scans_return_the_same_candidates() {
    let memory = SyntheticMemory::from_i32(&[7, 1, 7, 2, 7, 3, 7, 4]);
    let mut serial_options = options();
    serial_options.worker_threads = 1;
    let mut parallel_options = options();
    parallel_options.worker_threads = 4;

    let serial = first_scan(
        &memory,
        ValueType::I32,
        ScanPredicate::Exact(ScalarValue::Signed(7)),
        serial_options,
        &CancellationToken::new(),
    )
    .expect("serial scan should succeed");
    let parallel = first_scan(
        &memory,
        ValueType::I32,
        ScanPredicate::Exact(ScalarValue::Signed(7)),
        parallel_options,
        &CancellationToken::new(),
    )
    .expect("parallel scan should succeed");

    assert_eq!(serial.candidates, parallel.candidates);
}

#[test]
fn first_scan_rejects_history_predicates() {
    let memory = SyntheticMemory::from_i32(&[1, 2, 3]);
    let error = first_scan(
        &memory,
        ValueType::I32,
        ScanPredicate::Changed,
        options(),
        &CancellationToken::new(),
    )
    .expect_err("changed requires a previous snapshot");

    assert!(matches!(error, ScanError::FirstScanRequiresPrevious));
}

#[test]
fn result_limit_marks_scan_as_truncated() {
    let memory = SyntheticMemory::from_i32(&[1, 1, 1, 1]);
    let mut limited = options();
    limited.max_results = 2;
    let session = first_scan(
        &memory,
        ValueType::I32,
        ScanPredicate::Exact(ScalarValue::Signed(1)),
        limited,
        &CancellationToken::new(),
    )
    .expect("limited scan should succeed");

    assert_eq!(session.candidates.len(), 2);
    assert!(session.stats.truncated);
}

#[test]
fn cancellation_stops_scan_and_progress_is_reported() {
    let memory = SyntheticMemory::from_i32(&[1; 32]);
    let cancellation = CancellationToken::new();
    let cancellation_from_progress = cancellation.clone();
    let mut progress_events = 0_usize;

    let session = first_scan_with_progress(
        &memory,
        ValueType::I32,
        ScanPredicate::UnknownInitialValue,
        options(),
        &cancellation,
        |_| {
            progress_events += 1;
            cancellation_from_progress.cancel();
        },
    )
    .expect("cancelled scan should still return its partial session");

    assert!(session.stats.cancelled);
    assert!(progress_events >= 1);
    assert!(session.candidates.len() < 32);
}
