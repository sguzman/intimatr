# Performance and benchmarking

Intimatr keeps performance work reproducible instead of relying on one-off timing anecdotes. The repository includes a Criterion benchmark target and a Windows benchmark workflow for the scanner paths most likely to dominate ordinary memory-search workloads.

## Large-scan benchmark

`benches/scanner.rs` creates a deterministic 32 MiB synthetic readable/writable memory region and measures two full-region searches:

- a `u32` exact-value scalar first scan with four-byte alignment;
- a wildcard array-of-bytes scan using `DE AD ?? EF`.

Both use 1 MiB logical chunks and deliberately search for values that are absent from the synthetic region. That keeps result-vector growth from dominating the measurement and makes the benchmark primarily exercise traversal, decoding/predicate work, wildcard matching, chunk overlap, and `MemorySource::read_exact` traffic.

Run it locally on Windows with:

```powershell
cargo bench --bench scanner
```

For a fast smoke measurement matching CI:

```powershell
cargo bench --bench scanner -- --quick --noplot
```

Criterion reports throughput as bytes processed per second. Compare runs on the same machine and toolchain when evaluating a change; absolute numbers from GitHub-hosted runners are useful for regression orientation but are not stable hardware guarantees.

## Allocation/chunking hardening

Milestone 7 moves the scalar first scan and AOB scan away from allocating a fresh byte vector for every chunk. Each operation now owns one capacity-sized scratch buffer and resizes/reuses it while walking eligible regions. Result vectors also receive a bounded initial reservation instead of repeatedly growing from zero.

The important invariant is unchanged: chunk overlap still covers values/patterns that straddle a logical chunk boundary, alignment remains anchored to the memory-region traversal semantics, unreadable chunks remain isolated as read failures, and result ceilings/cancellation behavior are unchanged.

This optimization intentionally stays below the `MemorySource` abstraction. Synthetic benchmarks and `CurrentProcessMemory` exercise the same scanner implementation, so performance changes do not introduce a Windows-only second scanner.

## Parallel scalar scans

Field testing against a large real-world process exposed that the original scalar first/next scan path still traversed work serially even though the command dispatcher itself had multiple workers. Scalar scans now split first-scan chunks and next-scan candidate batches across a dedicated scanner worker group.

`scanner.worker_threads` controls that group independently from `[runtime].command_workers`. `0` selects automatic parallelism (capped at 16 workers); explicit values from 1 through 64 force a per-target scanner worker count. Result candidates are sorted by address after worker merge so ordinary non-truncated scans retain stable ordering.

The shared command layer now exposes active scan progress snapshots. Native UI progress polling therefore does not bypass policy or create a second scanner implementation, and first scans can be cancelled through the same cancellation token path as next scans.

## Backpressure and memory pressure

Runtime command work is routed through the bounded shared command executor. The worker count and queue capacity live in each target's TOML under `[runtime]`. When producers outpace the configured queue, submission blocks instead of spawning an unbounded amount of command work inside the target process.

RPC already bounds clients and frame sizes. The Windows debugger event feed is a fixed-capacity sequence-numbered ring, and the native UIs bound duplicate outstanding operations by task kind. The shared executor is therefore the common pressure boundary for command execution regardless of which frontend produced the request.

## Interpreting future optimization work

Before changing scanner algorithms, add or extend a benchmark that represents the suspected hot path. Useful future cases include dense result sets, unknown-initial scans, large historical next scans, many small regions, and pointer-search workloads. Preserve correctness tests around region boundaries, overlap, alignment, cancellation, snapshots, and read failures whenever traversal code changes.
