use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, trace, warn};

use crate::{
    config::ScannerConfig,
    memory::{MemoryError, MemorySource, RegionFilter},
};

use super::{PredicateError, ScalarValue, ScanPredicate, ValueError, ValueType};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanOptions {
    pub chunk_size_bytes: usize,
    pub alignment: usize,
    pub max_results: usize,
    pub float_epsilon: f64,
    pub region_filter: RegionFilterConfig,
}

impl From<&ScannerConfig> for ScanOptions {
    fn from(config: &ScannerConfig) -> Self {
        Self {
            chunk_size_bytes: config.chunk_size_bytes,
            alignment: config.alignment,
            max_results: config.max_results,
            float_epsilon: config.float_epsilon,
            region_filter: RegionFilterConfig {
                require_readable: config.require_readable,
                require_writable: config.require_writable,
                require_executable: config.require_executable,
                include_guard_pages: config.include_guard_pages,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionFilterConfig {
    pub require_readable: bool,
    pub require_writable: bool,
    pub require_executable: bool,
    pub include_guard_pages: bool,
}

impl From<RegionFilterConfig> for RegionFilter {
    fn from(value: RegionFilterConfig) -> Self {
        Self {
            require_readable: value.require_readable,
            require_writable: value.require_writable,
            require_executable: value.require_executable,
            include_guard_pages: value.include_guard_pages,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanCandidate {
    pub address: usize,
    pub current: ScalarValue,
    pub previous: Option<ScalarValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanSession {
    pub value_type: ValueType,
    pub candidates: Vec<ScanCandidate>,
    pub stats: ScanStats,
}

impl ScanSession {
    pub fn next_scan<S: MemorySource + ?Sized>(
        &self,
        source: &S,
        predicate: ScanPredicate,
        options: ScanOptions,
        cancellation: &CancellationToken,
    ) -> Result<Self, ScanError> {
        self.next_scan_with_progress(source, predicate, options, cancellation, |_| {})
    }

    pub fn next_scan_with_progress<S, F>(
        &self,
        source: &S,
        predicate: ScanPredicate,
        options: ScanOptions,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<Self, ScanError>
    where
        S: MemorySource + ?Sized,
        F: FnMut(ScanProgress),
    {
        validate_options(options)?;
        let started = Instant::now();
        let width = self.value_type.byte_width();
        let total_bytes = (self.candidates.len() as u64).saturating_mul(width as u64);
        let mut stats = ScanStats {
            regions_considered: 0,
            regions_scanned: 0,
            bytes_scanned: 0,
            bytes_read: 0,
            candidates_evaluated: 0,
            read_failures: 0,
            truncated: false,
            cancelled: false,
            elapsed_micros: 0,
            throughput_mib_per_sec: 0.0,
        };
        let mut candidates = Vec::with_capacity(self.candidates.len().min(options.max_results));
        let mut bytes = [0_u8; 8];

        info!(
            previous_results = self.candidates.len(),
            ?predicate,
            ?self.value_type,
            "starting next memory scan"
        );

        for (index, previous) in self.candidates.iter().enumerate() {
            if cancellation.is_cancelled() {
                stats.cancelled = true;
                break;
            }

            match source.read_exact(previous.address, &mut bytes[..width]) {
                Ok(()) => {
                    stats.bytes_read = stats.bytes_read.saturating_add(width as u64);
                    let current = self.value_type.decode(&bytes[..width])?;
                    stats.candidates_evaluated = stats.candidates_evaluated.saturating_add(1);
                    if predicate.matches(current, Some(previous.current), options.float_epsilon)? {
                        candidates.push(ScanCandidate {
                            address: previous.address,
                            current,
                            previous: Some(previous.current),
                        });
                        if candidates.len() >= options.max_results {
                            stats.truncated = true;
                            break;
                        }
                    }
                }
                Err(error) => {
                    stats.read_failures = stats.read_failures.saturating_add(1);
                    trace!(
                        address = previous.address,
                        error = %error,
                        "skipping next-scan candidate that could not be read"
                    );
                }
            }

            stats.bytes_scanned = stats.bytes_scanned.saturating_add(width as u64);
            if index % 4096 == 0 {
                progress(ScanProgress {
                    bytes_scanned: stats.bytes_scanned,
                    total_bytes,
                    results: candidates.len(),
                    read_failures: stats.read_failures,
                });
            }
        }

        finish_stats(&mut stats, started);
        progress(ScanProgress {
            bytes_scanned: stats.bytes_scanned,
            total_bytes,
            results: candidates.len(),
            read_failures: stats.read_failures,
        });

        if stats.read_failures > 0 {
            warn!(
                read_failures = stats.read_failures,
                "next scan skipped unreadable candidates"
            );
        }
        info!(
            results = candidates.len(),
            cancelled = stats.cancelled,
            truncated = stats.truncated,
            elapsed_micros = stats.elapsed_micros,
            throughput_mib_per_sec = stats.throughput_mib_per_sec,
            "next memory scan completed"
        );

        Ok(Self {
            value_type: self.value_type,
            candidates,
            stats,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ScanStats {
    pub regions_considered: u64,
    pub regions_scanned: u64,
    pub bytes_scanned: u64,
    pub bytes_read: u64,
    pub candidates_evaluated: u64,
    pub read_failures: u64,
    pub truncated: bool,
    pub cancelled: bool,
    pub elapsed_micros: u64,
    pub throughput_mib_per_sec: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub bytes_scanned: u64,
    pub total_bytes: u64,
    pub results: usize,
    pub read_failures: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub fn first_scan<S: MemorySource + ?Sized>(
    source: &S,
    value_type: ValueType,
    predicate: ScanPredicate,
    options: ScanOptions,
    cancellation: &CancellationToken,
) -> Result<ScanSession, ScanError> {
    first_scan_with_progress(source, value_type, predicate, options, cancellation, |_| {})
}

pub fn first_scan_with_progress<S, F>(
    source: &S,
    value_type: ValueType,
    predicate: ScanPredicate,
    options: ScanOptions,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<ScanSession, ScanError>
where
    S: MemorySource + ?Sized,
    F: FnMut(ScanProgress),
{
    validate_options(options)?;
    if predicate.requires_previous() {
        return Err(ScanError::FirstScanRequiresPrevious);
    }

    let started = Instant::now();
    let width = value_type.byte_width();
    let scan_span = aligned_scan_span(options.chunk_size_bytes, options.alignment)?;
    let filter: RegionFilter = options.region_filter.into();
    let mut regions = source.regions()?;
    regions.sort_unstable_by_key(|region| region.base);
    let total_bytes = regions
        .iter()
        .copied()
        .filter(|region| region.is_scannable(filter))
        .fold(0_u64, |total, region| {
            total.saturating_add(region.size as u64)
        });
    let mut stats = ScanStats {
        regions_considered: regions.len() as u64,
        ..ScanStats::default()
    };
    let mut candidates = Vec::with_capacity(options.max_results.min(4096));
    let mut buffer = Vec::with_capacity(
        options
            .chunk_size_bytes
            .saturating_add(width.saturating_sub(1)),
    );

    info!(
        ?value_type,
        ?predicate,
        alignment = options.alignment,
        chunk_size_bytes = options.chunk_size_bytes,
        max_results = options.max_results,
        total_bytes,
        "starting first memory scan"
    );

    'regions: for region in regions {
        if !region.is_scannable(filter) {
            trace!(
                base = region.base,
                size = region.size,
                "skipping memory region due to scan filter"
            );
            continue;
        }

        let region_end = region.end()?;
        stats.regions_scanned = stats.regions_scanned.saturating_add(1);
        debug!(
            base = region.base,
            size = region.size,
            readable = region.readable,
            writable = region.writable,
            executable = region.executable,
            guard = region.guard,
            "scanning memory region"
        );

        let mut cursor = region.base;
        while cursor
            .checked_add(width)
            .is_some_and(|end| end <= region_end)
        {
            if cancellation.is_cancelled() {
                stats.cancelled = true;
                break 'regions;
            }

            let remaining = region_end - cursor;
            let logical_span = scan_span.min(remaining);
            let overlap = width.saturating_sub(1);
            let read_len = remaining.min(logical_span.saturating_add(overlap));
            buffer.resize(read_len, 0);

            match source.read_exact(cursor, &mut buffer) {
                Ok(()) => {
                    stats.bytes_read = stats.bytes_read.saturating_add(read_len as u64);
                    let mut offset = 0_usize;
                    while offset < logical_span && offset.saturating_add(width) <= read_len {
                        if cancellation.is_cancelled() {
                            stats.cancelled = true;
                            break 'regions;
                        }

                        let current = value_type.decode(&buffer[offset..offset + width])?;
                        stats.candidates_evaluated = stats.candidates_evaluated.saturating_add(1);
                        if predicate.matches(current, None, options.float_epsilon)? {
                            candidates.push(ScanCandidate {
                                address: cursor + offset,
                                current,
                                previous: None,
                            });
                            if candidates.len() >= options.max_results {
                                stats.truncated = true;
                                break 'regions;
                            }
                        }

                        offset = offset
                            .checked_add(options.alignment)
                            .ok_or(ScanError::AddressOverflow)?;
                    }
                }
                Err(error) => {
                    stats.read_failures = stats.read_failures.saturating_add(1);
                    warn!(
                        address = cursor,
                        read_len,
                        error = %error,
                        "skipping unreadable scan chunk"
                    );
                }
            }

            stats.bytes_scanned = stats.bytes_scanned.saturating_add(logical_span as u64);
            progress(ScanProgress {
                bytes_scanned: stats.bytes_scanned,
                total_bytes,
                results: candidates.len(),
                read_failures: stats.read_failures,
            });

            cursor = cursor
                .checked_add(logical_span)
                .ok_or(ScanError::AddressOverflow)?;
        }
    }

    finish_stats(&mut stats, started);
    progress(ScanProgress {
        bytes_scanned: stats.bytes_scanned,
        total_bytes,
        results: candidates.len(),
        read_failures: stats.read_failures,
    });
    info!(
        results = candidates.len(),
        regions_scanned = stats.regions_scanned,
        candidates_evaluated = stats.candidates_evaluated,
        read_failures = stats.read_failures,
        cancelled = stats.cancelled,
        truncated = stats.truncated,
        elapsed_micros = stats.elapsed_micros,
        throughput_mib_per_sec = stats.throughput_mib_per_sec,
        "first memory scan completed"
    );

    Ok(ScanSession {
        value_type,
        candidates,
        stats,
    })
}

fn validate_options(options: ScanOptions) -> Result<(), ScanError> {
    if options.chunk_size_bytes == 0 {
        return Err(ScanError::InvalidOptions(
            "chunk_size_bytes must be non-zero",
        ));
    }
    if options.alignment == 0 {
        return Err(ScanError::InvalidOptions("alignment must be non-zero"));
    }
    if options.max_results == 0 {
        return Err(ScanError::InvalidOptions("max_results must be non-zero"));
    }
    if !options.float_epsilon.is_finite() || options.float_epsilon < 0.0 {
        return Err(ScanError::InvalidOptions(
            "float_epsilon must be finite and non-negative",
        ));
    }
    Ok(())
}

fn aligned_scan_span(chunk_size: usize, alignment: usize) -> Result<usize, ScanError> {
    let units = (chunk_size / alignment).max(1);
    units
        .checked_mul(alignment)
        .ok_or(ScanError::AddressOverflow)
}

fn finish_stats(stats: &mut ScanStats, started: Instant) {
    let elapsed = started.elapsed();
    stats.elapsed_micros = elapsed.as_micros().min(u64::MAX as u128) as u64;
    let seconds = elapsed.as_secs_f64();
    stats.throughput_mib_per_sec = if seconds > 0.0 {
        (stats.bytes_scanned as f64 / (1024.0 * 1024.0)) / seconds
    } else {
        0.0
    };
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Predicate(#[from] PredicateError),
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error("first scan cannot use a predicate that requires previous values")]
    FirstScanRequiresPrevious,
    #[error("invalid scan options: {0}")]
    InvalidOptions(&'static str),
    #[error("address arithmetic overflowed")]
    AddressOverflow,
}
