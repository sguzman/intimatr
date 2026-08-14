# Intimatr Milestones

Work is ordered by dependency: later phases should build on the shared core instead of bypassing it.

## 0. Foundation

- [x] Convert the crate into a Rust `cdylib`/`rlib` core suitable for producing `intimatr.dll`.
- [x] Define the one-TOML-per-executable configuration convention.
- [x] Add validated configuration models for target, logging, RPC, scanner, debugger, UI, and policy settings.
- [x] Add non-blocking structured file logging with optional console output.
- [x] Define CE-style scan predicates: exact, comparison, range, changed/unchanged, increased/decreased, and delta predicates.
- [x] Add tests for configuration resolution/validation and predicate semantics.
- [x] Add CI for formatting, Clippy, and tests.
- [x] Add initial project specification and build documentation.

## 1. Windows DLL bootstrap and lifecycle

- [x] Add the minimal Windows DLL entrypoint and keep loader-lock work trivial.
- [x] Move initialization onto a dedicated bootstrap thread after process attach.
- [x] Resolve the host executable path and automatically load its per-game TOML configuration.
- [x] Initialize tracing before subsystem startup and log lifecycle transitions extensively.
- [x] Add panic containment around FFI/thread entry boundaries.
- [x] Add explicit lifecycle states and a clean shutdown path.
- [x] Add lifecycle/path-resolution tests and keep Windows FFI isolated behind auditable modules.

## 2. Memory engine and CE-style scanner

- [x] Enumerate committed memory regions with `VirtualQuery` and normalize region metadata.
- [x] Apply readable/writable/executable/guard-page filters from configuration.
- [x] Add safe wrappers for typed in-process reads.
- [x] Add policy-gated typed writes with temporary protection changes where needed.
- [x] Implement first-scan traversal with chunking and alignment controls.
- [x] Store typed scan candidates and previous-value snapshots.
- [x] Implement next-scan filtering using all predicates already defined in `scanner::predicate`.
- [x] Support signed integers, unsigned integers, `f32`, and `f64` with explicit data-width metadata.
- [x] Add unknown-initial-value scans for CE-style discovery workflows.
- [x] Add scan cancellation, result limits, progress reporting, and timing/throughput logs.
- [x] Add deterministic scanner tests over synthetic memory buffers plus Windows backend smoke tests.

## 3. Shared command layer and RPC

- [x] Define frontend-neutral commands/results for modules, memory, scans, watches, debugger operations, and lifecycle.
- [x] Enforce policy gates in the shared command layer rather than individually in frontends.
- [x] Define a versioned serialized RPC protocol.
- [x] Implement framed localhost TCP transport.
- [x] Implement optional Windows named-pipe transport.
- [x] Add connection/request size limits and structured per-request tracing spans.
- [x] Add RPC integration tests using an in-memory or loopback client/server pair.
- [x] Add a small first-party Rust client crate/example to prove arbitrary external frontend support.

## 4. In-process CE-style UI

- [ ] Select and integrate the Rust GUI/rendering stack without doing heavy work in `DllMain`.
- [ ] Add overlay visibility/hotkey handling from TOML.
- [ ] Add memory scan controls and result table.
- [ ] Add watch/freeze list backed by the shared command layer.
- [ ] Add memory viewer/editor.
- [ ] Add module/thread browser.
- [ ] Add persistent UI layout/state per target executable where useful.

## 5. Debugger core and UI

- [ ] Enumerate threads and expose register/context snapshots.
- [ ] Integrate an x86/x64 disassembler.
- [ ] Add instruction/memory views and navigation.
- [ ] Implement hardware breakpoint management.
- [ ] Implement exception-based breakpoint handling with carefully scoped VEH logic.
- [ ] Add continue/pause/single-step state machinery where viable in-process.
- [ ] Surface debugger events through both the UI and RPC stream.
- [ ] Add debugger-focused tests around state transitions and breakpoint bookkeeping.

## 6. Advanced search and analysis

- [ ] Add array-of-bytes/pattern scanning with wildcards.
- [ ] Add pointer-chain search and validation.
- [ ] Add module-relative/symbolic address expressions.
- [ ] Add saved scan sessions and reusable watch definitions.
- [ ] Add structure-oriented memory inspection helpers.
- [ ] Add scriptable automation through the RPC command API without duplicating core logic.

## 7. Hardening and releases

- [ ] Benchmark large scans and optimize allocation/chunking hot paths.
- [ ] Add bounded queues/backpressure between scanner, UI, debugger, and RPC tasks.
- [ ] Add crash-safe shutdown and log flushing.
- [ ] Add Windows x86_64 CI builds that verify the DLL artifact.
- [ ] Add release packaging with example configs and protocol/version notes.
- [ ] Document frontend/plugin extension points and subsystem invariants.
