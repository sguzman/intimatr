use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use intimatr::{
    analysis::{PatternScanOptions, scan_pattern},
    memory::{MemoryError, MemoryRegion, MemorySource},
    scanner::{
        CancellationToken, RegionFilterConfig, ScalarValue, ScanOptions, ScanPredicate, ValueType,
        first_scan,
    },
};

const BASE: usize = 0x1000_0000;
// Keep this workload stable so Milestone 7 throughput runs remain directly comparable.
const LARGE_SCAN_BYTES: usize = 32 * 1024 * 1024;

struct SyntheticMemory {
    bytes: Vec<u8>,
}

impl SyntheticMemory {
    fn large() -> Self {
        Self {
            bytes: vec![0xA5; LARGE_SCAN_BYTES],
        }
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
        let source = self
            .bytes
            .get(offset..end)
            .ok_or(MemoryError::RegionNotFound {
                address,
                size: buffer.len(),
            })?;
        buffer.copy_from_slice(source);
        Ok(())
    }
}

fn scanner_options() -> ScanOptions {
    ScanOptions {
        chunk_size_bytes: 1024 * 1024,
        alignment: 4,
        max_results: 4096,
        float_epsilon: 1.0e-6,
        region_filter: RegionFilterConfig {
            require_readable: true,
            require_writable: false,
            require_executable: false,
            include_guard_pages: false,
        },
    }
}

fn scanner_config() -> intimatr::config::ScannerConfig {
    intimatr::config::ScannerConfig {
        chunk_size_bytes: 1024 * 1024,
        alignment: 1,
        max_results: 4096,
        float_epsilon: 1.0e-6,
        require_readable: true,
        require_writable: false,
        require_executable: false,
        include_guard_pages: false,
    }
}

fn large_scan_benchmarks(c: &mut Criterion) {
    let memory = SyntheticMemory::large();
    let cancellation = CancellationToken::new();
    let mut group = c.benchmark_group("large_scan");
    group.throughput(Throughput::Bytes(LARGE_SCAN_BYTES as u64));

    group.bench_function("scalar_u32_exact_32_mib", |bencher| {
        bencher.iter(|| {
            let session = first_scan(
                &memory,
                ValueType::U32,
                ScanPredicate::Exact(ScalarValue::Unsigned(0xDEAD_BEEF)),
                scanner_options(),
                &cancellation,
            )
            .expect("large scalar benchmark scan should succeed");
            black_box(session.candidates.len())
        });
    });

    group.bench_function("aob_wildcard_32_mib", |bencher| {
        bencher.iter(|| {
            let result = scan_pattern(
                &memory,
                "DE AD ?? EF",
                &scanner_config(),
                PatternScanOptions {
                    alignment: 1,
                    max_results: 4096,
                },
            )
            .expect("large AOB benchmark scan should succeed");
            black_box(result.addresses.len())
        });
    });

    group.finish();
}

criterion_group!(benches, large_scan_benchmarks);
criterion_main!(benches);
