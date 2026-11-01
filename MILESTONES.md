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

- [x] Select and integrate the Rust GUI/rendering stack without doing heavy work in `DllMain`.
- [x] Add native tool-window visibility/hotkey handling from TOML.
- [x] Add memory scan controls and result table.
- [x] Add watch/freeze list backed by the shared command layer.
- [x] Add memory viewer/editor.
- [x] Add module/thread browser.
- [x] Add persistent UI layout/state per target executable where useful.

## 5. Debugger core and UI

- [x] Enumerate threads and expose register/context snapshots.
- [x] Integrate an x86/x64 disassembler.
- [x] Add instruction/memory views and navigation.
- [x] Implement hardware breakpoint management.
- [x] Implement exception-based breakpoint handling with carefully scoped VEH logic.
- [x] Add continue/pause/single-step state machinery where viable in-process.
- [x] Surface debugger events through both the UI and RPC stream.
- [x] Add debugger-focused tests around state transitions and breakpoint bookkeeping.

## 6. Advanced search and analysis

- [x] Add array-of-bytes/pattern scanning with wildcards.
- [x] Add pointer-chain search and validation.
- [x] Add module-relative/symbolic address expressions.
- [x] Add saved scan sessions and reusable watch definitions.
- [x] Add structure-oriented memory inspection helpers.
- [x] Add scriptable automation through the RPC command API without duplicating core logic.

## 7. Hardening and releases

- [x] Benchmark large scans and optimize allocation/chunking hot paths.
- [x] Add bounded queues/backpressure between scanner, UI, debugger, and RPC tasks.
- [x] Add crash-safe shutdown and log flushing.
- [x] Add Windows x86_64 CI builds that verify the DLL artifact.
- [x] Add release packaging with example configs and protocol/version notes.
- [x] Document frontend/plugin extension points and subsystem invariants.

## 8. Release candidate usability

- [x] Add an archive verifier that checks release layout, version metadata, and the packaged DLL checksum.
- [x] Make CI validate the exact release ZIP produced by the packaging script.
- [x] Publish an archive-level SHA-256 sidecar alongside tagged release ZIPs.
- [x] Add a first-run quick start from package layout through configuration, UI scanning, RPC, persistence, and shutdown.
- [x] Add troubleshooting guidance for bootstrap, configuration, UI/debugger policy gates, scanning, RPC, stale addresses, and lifecycle behavior.

## 9. Field-test scan hardening

- [x] Parallelize scalar first scans across memory chunks with a dedicated configurable worker group.
- [x] Parallelize historical next scans across candidate batches while preserving sorted result ordering.
- [x] Expose live active-scan progress through the shared command layer.
- [x] Show live progress/throughput and a persistent running-scan indicator in the native UI.
- [x] Allow cancellation of both first and next scans from the native UI.
- [x] Keep unrelated UI errors from obscuring whether a scan is still active.
- [x] Isolate every process run into its own diagnostic log file so field-test logs never append across launches.

## 10. Speed-control expansion — feasibility and design 🔍
**Status:** active (3/9 points verified). **Weight:** 9/100 expansion points.
- [x] INT-1001 🔎 Inventory regression baseline and integration seams.
- [ ] INT-1002 ⏱️ Prove timing-hook strategy and clock continuity contract.
- [ ] INT-1003 🍷 Prove Bottles/Wine loader and configuration-path strategy.

## 11. Time virtualization engine ⏩
**Status:** pending. **Weight:** 19/100 expansion points.
- [ ] INT-1101 🧮 Pure virtual clock model.
- [ ] INT-1102 🪝 Windows timing-hook backend.
- [ ] INT-1103 🧭 Clock consistency and live multiplier transitions.
- [ ] INT-1104 🧯 Safe hook lifecycle and fallback.
- [ ] INT-1105 🎮 Offline Wine game proof.

## 12. Shared configuration and profiles ⚙️
**Status:** pending. **Weight:** 12/100 expansion points.
- [ ] INT-1201 🗂️ Global versioned speed profile.
- [ ] INT-1202 🧩 Per-bottle/per-game matching.
- [ ] INT-1203 🔗 Wine-visible shared configuration path.
- [ ] INT-1204 🔄 Reload, validation, persistence.

## 13. Hotkeys and speed modulation ⌨️
**Status:** pending. **Weight:** 10/100 expansion points.
- [ ] INT-1301 🎚️ Step/preset/toggle/reset state machine.
- [ ] INT-1302 ✋ Hold-to-modulate and optional startup speed.
- [ ] INT-1303 🔑 Configurable, conflict-aware hotkeys.
- [ ] INT-1304 🧷 Authoritative status and recovery.

## 14. Bottles deployment manager 🍾
**Status:** pending. **Weight:** 23/100 expansion points.
- [ ] INT-1401 🔭 Discover native Bottles and games.
- [ ] INT-1402 🧲 Include/exclude filters.
- [ ] INT-1403 📋 Non-mutating plan; apply/status operations.
- [ ] INT-1404 🔌 Verified normal-launch DLL loading.
- [ ] INT-1405 🛟 Idempotence, backups and rollback.
- [ ] INT-1406 🧱 32-/64-bit and runner compatibility reporting.

## 15. Unified UI/RPC 🖥️
**Status:** pending. **Weight:** 7/100 expansion points.
- [ ] INT-1501 🧠 Shared command and RPC interface.
- [ ] INT-1502 🪟 Native egui controls and appearance/keyboard QA.
- [ ] INT-1503 📟 Status without keeping the scanner foregrounded.

## 16. Verification and hardening 🧪
**Status:** pending. **Weight:** 15/100 expansion points.
- [ ] INT-1601 🧫 Deterministic core/config/input tests.
- [ ] INT-1602 🍷 Real Bottles/Wine compatibility matrix.
- [ ] INT-1603 🩺 Scanner/debugger/RPC regression evidence.
- [ ] INT-1604 🔐 Failure recovery and rollback QA.

## 17. Documentation and acceptance 📦
**Status:** pending. **Weight:** 5/100 expansion points.
- [ ] INT-1701 📚 Configuration/usage/troubleshooting reference.
- [ ] INT-1702 📦 Verified distributables and CI/package contents.
- [ ] INT-1703 🏁 End-to-end owner acceptance checkpoint.

**Expanded roadmap and precise task acceptance criteria:** [Speed Control and Bottles Integration Roadmap](docs/speed-control-roadmap.md).

**Progress basis:** Milestones 0–9 retain their historical checked state and are not included in the new 100-point expansion denominator. All milestone 10–17 deliverables started pending. The repository baseline inspection INT-1001 is verified against source and historical CI, earning 3/100 weighted points; expansion progress is **3%** (previous 0%, delta +3 percentage points). No speed-control runtime implementation is verified. Planning/documentation alone does not complete the features. Confirmed changes must satisfy each leaf item's tests and evidence before gaining weighted credit.
