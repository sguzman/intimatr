use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
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

const MAX_SCANNER_WORKERS: usize = 64;
const AUTO_SCANNER_WORKER_CAP: usize = 16;
const NEXT_SCAN_BATCH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanOptions {
    pub chunk_size_bytes: usize,
    /// `0` selects an automatic worker count based on available parallelism.
    pub worker_threads: usize,
    pub alignment: usize,
    pub max_results: usize,
    pub float_epsilon: f64,
    pub region_filter: RegionFilterConfig,
}

impl From<&ScannerConfig> for ScanOptions {
    fn from(config: &ScannerConfig) -> Self {
        Self {
            chunk_size_bytes: config.chunk_size_bytes,
            worker_threads: config.worker_threads,
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
    pub fn next_scan<S: MemorySource + Sync + ?Sized>(
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
        S: MemorySource + Sync + ?Sized,
        F: FnMut(ScanProgress),
    {
        validate_options(options)?;
        let started = Instant::now();
        let width = self.value_type.byte_width();
        let total_bytes = (self.candidates.len() as u64).saturating_mul(width as u64);
        let worker_count = resolve_worker_count(options.worker_threads, self.candidates.len());

        info!(
            previous_results = self.candidates.len(),
            ?predicate,
            ?self.value_type,
            worker_count,
            "starting next memory scan"
        );

        if self.candidates.is_empty() {
            let mut stats = ScanStats::default();
            finish_stats(&mut stats, started);
            progress(ScanProgress {
                bytes_scanned: 0,
                total_bytes,
                results: 0,
                read_failures: 0,
            });
            return Ok(Self {
                value_type: self.value_type,
                candidates: Vec::new(),
                stats,
            });
        }

        let next_index = AtomicUsize::new(0);
        let bytes_scanned = AtomicU64::new(0);
        let bytes_read = AtomicU64::new(0);
        let candidates_evaluated = AtomicU64::new(0);
        let read_failures = AtomicU64::new(0);
        let accepted_results = AtomicUsize::new(0);
        let truncated = AtomicBool::new(false);
        let stop = AtomicBool::new(false);

        let mut candidates =
            thread::scope(|scope| -> Result<Vec<ScanCandidate>, ScanError> {
                let (event_tx, event_rx) = mpsc::sync_channel(0);
                let mut handles = Vec::with_capacity(worker_count);

                for _ in 0..worker_count {
                    let event_tx = event_tx.clone();
                    let session_candidates = &self.candidates;
                    let next_index = &next_index;
                    let bytes_scanned = &bytes_scanned;
                    let bytes_read = &bytes_read;
                    let candidates_evaluated = &candidates_evaluated;
                    let read_failures = &read_failures;
                    let accepted_results = &accepted_results;
                    let truncated = &truncated;
                    let stop = &stop;

                    handles.push(scope.spawn(move || -> Result<Vec<ScanCandidate>, ScanError> {
                    let _done = WorkerDone(event_tx.clone());
                    let mut local = Vec::new();
                    let mut bytes = [0_u8; 8];

                    loop {
                        if cancellation.is_cancelled() || stop.load(Ordering::Acquire) {
                            break;
                        }
                        let start = next_index.fetch_add(NEXT_SCAN_BATCH, Ordering::Relaxed);
                        if start >= session_candidates.len() {
                            break;
                        }
                        let end = start
                            .saturating_add(NEXT_SCAN_BATCH)
                            .min(session_candidates.len());

                        for previous in &session_candidates[start..end] {
                            if cancellation.is_cancelled() || stop.load(Ordering::Acquire) {
                                break;
                            }

                            match source.read_exact(previous.address, &mut bytes[..width]) {
                                Ok(()) => {
                                    bytes_read.fetch_add(width as u64, Ordering::Relaxed);
                                    let current = self.value_type.decode(&bytes[..width])?;
                                    candidates_evaluated.fetch_add(1, Ordering::Relaxed);
                                    if predicate.matches(
                                        current,
                                        Some(previous.current),
                                        options.float_epsilon,
                                    )? && reserve_result_slot(
                                        accepted_results,
                                        options.max_results,
                                        truncated,
                                        stop,
                                    ) {
                                        local.push(ScanCandidate {
                                            address: previous.address,
                                            current,
                                            previous: Some(previous.current),
                                        });
                                    }
                                }
                                Err(error) => {
                                    read_failures.fetch_add(1, Ordering::Relaxed);
                                    trace!(
                                        address = previous.address,
                                        error = %error,
                                        "skipping next-scan candidate that could not be read"
                                    );
                                }
                            }
                            bytes_scanned.fetch_add(width as u64, Ordering::Relaxed);
                        }

                        let _ = event_tx.send(WorkerEvent::Progress);
                    }

                    Ok(local)
                }));
                }
                drop(event_tx);

                drain_worker_progress(
                    &event_rx,
                    worker_count,
                    total_bytes,
                    options.max_results,
                    &bytes_scanned,
                    &accepted_results,
                    &read_failures,
                    &mut progress,
                );

                let mut merged = Vec::new();
                for handle in handles {
                    let local = handle.join().map_err(|_| ScanError::WorkerPanicked)??;
                    merged.extend(local);
                }
                Ok(merged)
            })?;

        candidates.sort_unstable_by_key(|candidate| candidate.address);
        candidates.truncate(options.max_results);

        let mut stats = ScanStats {
            regions_considered: 0,
            regions_scanned: 0,
            bytes_scanned: bytes_scanned.load(Ordering::Acquire),
            bytes_read: bytes_read.load(Ordering::Acquire),
            candidates_evaluated: candidates_evaluated.load(Ordering::Acquire),
            read_failures: read_failures.load(Ordering::Acquire),
            truncated: truncated.load(Ordering::Acquire),
            cancelled: cancellation.is_cancelled(),
            elapsed_micros: 0,
            throughput_mib_per_sec: 0.0,
        };
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
            worker_count,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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

pub fn first_scan<S: MemorySource + Sync + ?Sized>(
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
    S: MemorySource + Sync + ?Sized,
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

    let mut chunks = Vec::new();
    for (region_index, region) in regions.iter().copied().enumerate() {
        if !region.is_scannable(filter) {
            trace!(
                base = region.base,
                size = region.size,
                "skipping memory region due to scan filter"
            );
            continue;
        }

        let region_end = region.end()?;
        debug!(
            base = region.base,
            size = region.size,
            readable = region.readable,
            writable = region.writable,
            executable = region.executable,
            guard = region.guard,
            "queueing memory region for parallel scan"
        );

        let mut cursor = region.base;
        while cursor
            .checked_add(width)
            .is_some_and(|end| end <= region_end)
        {
            let remaining = region_end - cursor;
            let logical_span = scan_span.min(remaining);
            let overlap = width.saturating_sub(1);
            let read_len = remaining.min(logical_span.saturating_add(overlap));
            chunks.push(ScanChunk {
                region_index,
                address: cursor,
                logical_span,
                read_len,
            });
            cursor = cursor
                .checked_add(logical_span)
                .ok_or(ScanError::AddressOverflow)?;
        }
    }

    let worker_count = resolve_worker_count(options.worker_threads, chunks.len());
    info!(
        ?value_type,
        ?predicate,
        alignment = options.alignment,
        chunk_size_bytes = options.chunk_size_bytes,
        worker_count,
        max_results = options.max_results,
        total_bytes,
        chunks = chunks.len(),
        "starting first memory scan"
    );

    if chunks.is_empty() {
        let mut stats = ScanStats {
            regions_considered: regions.len() as u64,
            ..ScanStats::default()
        };
        finish_stats(&mut stats, started);
        progress(ScanProgress {
            bytes_scanned: 0,
            total_bytes,
            results: 0,
            read_failures: 0,
        });
        return Ok(ScanSession {
            value_type,
            candidates: Vec::new(),
            stats,
        });
    }

    let next_chunk = AtomicUsize::new(0);
    let bytes_scanned = AtomicU64::new(0);
    let bytes_read = AtomicU64::new(0);
    let candidates_evaluated = AtomicU64::new(0);
    let read_failures = AtomicU64::new(0);
    let regions_scanned = AtomicU64::new(0);
    let accepted_results = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let stop = AtomicBool::new(false);
    let region_started: Vec<_> = (0..regions.len()).map(|_| AtomicBool::new(false)).collect();

    let mut candidates = thread::scope(|scope| -> Result<Vec<ScanCandidate>, ScanError> {
        let (event_tx, event_rx) = mpsc::sync_channel(0);
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let event_tx = event_tx.clone();
            let chunks = &chunks;
            let next_chunk = &next_chunk;
            let bytes_scanned = &bytes_scanned;
            let bytes_read = &bytes_read;
            let candidates_evaluated = &candidates_evaluated;
            let read_failures = &read_failures;
            let regions_scanned = &regions_scanned;
            let accepted_results = &accepted_results;
            let truncated = &truncated;
            let stop = &stop;
            let region_started = &region_started;

            handles.push(
                scope.spawn(move || -> Result<Vec<ScanCandidate>, ScanError> {
                    let _done = WorkerDone(event_tx.clone());
                    let mut local = Vec::new();
                    let mut buffer = Vec::with_capacity(
                        options
                            .chunk_size_bytes
                            .saturating_add(width.saturating_sub(1)),
                    );

                    loop {
                        if cancellation.is_cancelled() || stop.load(Ordering::Acquire) {
                            break;
                        }
                        let chunk_index = next_chunk.fetch_add(1, Ordering::Relaxed);
                        let Some(chunk) = chunks.get(chunk_index).copied() else {
                            break;
                        };

                        if !region_started[chunk.region_index].swap(true, Ordering::AcqRel) {
                            regions_scanned.fetch_add(1, Ordering::Relaxed);
                        }
                        buffer.resize(chunk.read_len, 0);
                        let mut covered = 0_usize;

                        match source.read_exact(chunk.address, &mut buffer) {
                            Ok(()) => {
                                bytes_read.fetch_add(chunk.read_len as u64, Ordering::Relaxed);
                                let mut offset = 0_usize;
                                while offset < chunk.logical_span
                                    && offset.saturating_add(width) <= chunk.read_len
                                {
                                    if cancellation.is_cancelled() || stop.load(Ordering::Acquire) {
                                        break;
                                    }

                                    let current =
                                        value_type.decode(&buffer[offset..offset + width])?;
                                    candidates_evaluated.fetch_add(1, Ordering::Relaxed);
                                    covered = offset
                                        .saturating_add(options.alignment)
                                        .min(chunk.logical_span);
                                    if predicate.matches(current, None, options.float_epsilon)?
                                        && reserve_result_slot(
                                            accepted_results,
                                            options.max_results,
                                            truncated,
                                            stop,
                                        )
                                    {
                                        local.push(ScanCandidate {
                                            address: chunk.address + offset,
                                            current,
                                            previous: None,
                                        });
                                    }

                                    offset = offset
                                        .checked_add(options.alignment)
                                        .ok_or(ScanError::AddressOverflow)?;
                                }
                            }
                            Err(error) => {
                                read_failures.fetch_add(1, Ordering::Relaxed);
                                covered = chunk.logical_span;
                                warn!(
                                    address = chunk.address,
                                    read_len = chunk.read_len,
                                    error = %error,
                                    "skipping unreadable scan chunk"
                                );
                            }
                        }

                        bytes_scanned.fetch_add(covered as u64, Ordering::Relaxed);
                        let _ = event_tx.send(WorkerEvent::Progress);
                    }

                    Ok(local)
                }),
            );
        }
        drop(event_tx);

        drain_worker_progress(
            &event_rx,
            worker_count,
            total_bytes,
            options.max_results,
            &bytes_scanned,
            &accepted_results,
            &read_failures,
            &mut progress,
        );

        let mut merged = Vec::new();
        for handle in handles {
            let local = handle.join().map_err(|_| ScanError::WorkerPanicked)??;
            merged.extend(local);
        }
        Ok(merged)
    })?;

    candidates.sort_unstable_by_key(|candidate| candidate.address);
    candidates.truncate(options.max_results);

    let mut stats = ScanStats {
        regions_considered: regions.len() as u64,
        regions_scanned: regions_scanned.load(Ordering::Acquire),
        bytes_scanned: bytes_scanned.load(Ordering::Acquire),
        bytes_read: bytes_read.load(Ordering::Acquire),
        candidates_evaluated: candidates_evaluated.load(Ordering::Acquire),
        read_failures: read_failures.load(Ordering::Acquire),
        truncated: truncated.load(Ordering::Acquire),
        cancelled: cancellation.is_cancelled(),
        elapsed_micros: 0,
        throughput_mib_per_sec: 0.0,
    };
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
        worker_count,
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

#[derive(Clone, Copy)]
struct ScanChunk {
    region_index: usize,
    address: usize,
    logical_span: usize,
    read_len: usize,
}

enum WorkerEvent {
    Progress,
    Done,
}

struct WorkerDone(SyncSender<WorkerEvent>);

impl Drop for WorkerDone {
    fn drop(&mut self) {
        let _ = self.0.send(WorkerEvent::Done);
    }
}

#[allow(clippy::too_many_arguments)]
fn drain_worker_progress<F>(
    receiver: &mpsc::Receiver<WorkerEvent>,
    worker_count: usize,
    total_bytes: u64,
    max_results: usize,
    bytes_scanned: &AtomicU64,
    accepted_results: &AtomicUsize,
    read_failures: &AtomicU64,
    progress: &mut F,
) where
    F: FnMut(ScanProgress),
{
    let mut completed = 0_usize;
    while completed < worker_count {
        match receiver.recv() {
            Ok(WorkerEvent::Progress) => progress(ScanProgress {
                bytes_scanned: bytes_scanned.load(Ordering::Acquire),
                total_bytes,
                results: accepted_results.load(Ordering::Acquire).min(max_results),
                read_failures: read_failures.load(Ordering::Acquire),
            }),
            Ok(WorkerEvent::Done) => completed += 1,
            Err(_) => break,
        }
    }
}

fn reserve_result_slot(
    accepted_results: &AtomicUsize,
    max_results: usize,
    truncated: &AtomicBool,
    stop: &AtomicBool,
) -> bool {
    let slot = accepted_results.fetch_add(1, Ordering::AcqRel);
    if slot < max_results {
        if slot + 1 >= max_results {
            truncated.store(true, Ordering::Release);
            stop.store(true, Ordering::Release);
        }
        true
    } else {
        truncated.store(true, Ordering::Release);
        stop.store(true, Ordering::Release);
        false
    }
}

fn resolve_worker_count(configured: usize, work_items: usize) -> usize {
    let automatic = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(AUTO_SCANNER_WORKER_CAP);
    let requested = if configured == 0 {
        automatic
    } else {
        configured.min(MAX_SCANNER_WORKERS)
    };
    requested.max(1).min(work_items.max(1))
}

fn validate_options(options: ScanOptions) -> Result<(), ScanError> {
    if options.chunk_size_bytes == 0 {
        return Err(ScanError::InvalidOptions(
            "chunk_size_bytes must be non-zero",
        ));
    }
    if options.worker_threads > MAX_SCANNER_WORKERS {
        return Err(ScanError::InvalidOptions(
            "worker_threads must be 0 (auto) or at most 64",
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
    #[error("a scanner worker thread panicked")]
    WorkerPanicked,
}
