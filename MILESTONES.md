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
**Status:** active (6/9 points verified). **Weight:** 9/100 expansion points.
- [x] INT-1001 🔎 Inventory regression baseline and integration seams.
- [x] INT-1002 ⏱️ Prove timing-hook feasibility in isolated Wine QPC IAT test (0.500×/2.000×/1.000×, CI 41367449695; restricted fixture only).
- [ ] INT-1003 🍷 Prove Bottles/Wine loader and configuration-path strategy (active; isolated 64-bit Wine LoadLibrary smoke passed, live attach/path proof outstanding).

## 11. Time virtualization engine ⏩
**Status:** active (4/19 points verified). **Weight:** 19/100 expansion points.
- [x] INT-1101 🧮 Pure virtual clock model (Linux CI 41357696793, cross-target build and synthetic tests).
- [ ] INT-1102 🪝 Windows timing-hook backend.
- [ ] INT-1103 🧭 Clock consistency and live multiplier transitions (active; coordinated pure QPC/millisecond model verified, timing hooks pending).
- [ ] INT-1104 🧯 Safe hook lifecycle and fallback.
- [ ] INT-1105 🎮 Offline Wine game proof.

## 12. Shared configuration and profiles ⚙️
**Status:** active (3/12 points verified). **Weight:** 12/100 expansion points.
- [x] INT-1201 🗂️ Global versioned speed profile (validated TOML, Linux CI 41357980641).
- [ ] INT-1202 🧩 Per-bottle/per-game matching.
- [ ] INT-1203 🔗 Wine-visible shared configuration path.
- [ ] INT-1204 🔄 Reload, validation, persistence.

## 13. Hotkeys and speed modulation ⌨️
**Status:** active (3/10 points verified). **Weight:** 10/100 expansion points.
- [x] INT-1301 🎚️ Step/preset/toggle/reset state machine (Linux CI 41358317446 core unit tests).
- [ ] INT-1302 ✋ Hold-to-modulate and optional startup speed.
- [ ] INT-1303 🔑 Configurable, conflict-aware hotkeys.
- [ ] INT-1304 🧷 Authoritative status and recovery.

## 14. Bottles deployment manager 🍾
**Status:** active (0/23 points verified; native read-only inventory implemented, real Bottles QA pending). **Weight:** 23/100 expansion points.
- [ ] INT-1401 🔭 Discover native Bottles and games (active; parser/CLI code committed, real discovery not yet verified).
- [ ] INT-1402 🧲 Include/exclude filters (active; read-only selection CLI and synthetic fixture tests implemented, native Bottles field QA pending).
- [ ] INT-1403 📋 Non-mutating plan; apply/status operations.
- [ ] INT-1404 🔌 Verified normal-launch DLL loading.
- [ ] INT-1405 🛟 Idempotence, backups and rollback (including process attach).
- [ ] INT-1406 🧱 32-/64-bit, Linux GNU build and runner compatibility reporting.
- [ ] INT-1407 🧷 Explicit attach to already-running Wine processes via native Linux manager.

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
- [ ] INT-1702 📦 Linux-built Windows PE DLL, native Linux CLI, and verified CI/package contents.
- [ ] INT-1703 🏁 End-to-end owner acceptance checkpoint.

**Expanded roadmap and precise task acceptance criteria:** [Speed Control and Bottles Integration Roadmap](docs/speed-control-roadmap.md).

**Progress basis:** Milestones 0–9 retain their historical checked state and are not included in the new 100-point expansion denominator. All milestone 10–17 deliverables started pending. The repository baseline inspection INT-1001 is verified against source and historical CI, earning 3/100 weighted points; Linux-hosted PE build and explicit already-running Wine-process attachment are now firm requirements, with M14 reweighted to accommodate INT-1407 while retaining the 100-point overall denominator; expansion progress is **13%** (previous session 3%, delta +10 percentage points) after verifying INT-1101 pure clock, INT-1201 global speed profile and INT-1301 modulation state machine. INT-1401 has source and CI in progress but earns no weight until real native Bottles inventory is validated. The pure virtual clock and Linux-hosted x86-64 GNU DLL cross-build have been verified through CI; no Windows timer hook or Wine process attachment is verified. Planning/documentation alone does not complete the features. Confirmed changes must satisfy each leaf item's tests and evidence before gaining weighted credit.

**2026-11-02 continued development checkpoint:** timer API projection and shared speed session tests, safe Bottles selection plans, and an isolated `wine-load-probe` were added. [Wine smoke CI 41366257752](https://github.com/sguzman/intimatr/actions/runs/41366257752) verified that the Linux-built DLL loads and shuts down inside a disposable 64-bit Wine process. It does not verify an arbitrary game, runtime attachment, or speed manipulation. **Weighted verified expansion remains 13% (previous 13%, delta +0 pp).** See the detailed roadmap for active tasks and next steps.

**2026-11-02 timing-hook checkpoint:** INT-1002 is verified by the controlled Windows/Wine QPC IAT timing proof with original-import restoration; see [timing-design.md](docs/timing-design.md). **Expansion 16% (previous 13%, delta +3 percentage points).** This credits only hook feasibility, not production hooking, Bottles or full CI on current main.

**Three-API Wine integration note (2026-11-02):** the isolated CI fixture verified QPC/GetTickCount64/GetTickCount at 0.5×/2×/1× with three imports restored ([run 41368311733](https://github.com/sguzman/intimatr/actions/runs/41368311733)). INT-1102/1103 still require production-ready hook lifecycle and real compatibility evidence. **16% verified; delta +0 pp** after this additional uncredited test.

**Four-API Wine + rollback checkpoint (2026-11-02):** run [41370469163](https://github.com/sguzman/intimatr/actions/runs/41370469163) verified four import hooks (including WinMM), 0.5×/2×/1× speed transitions, rollback fault cases 0–3, clean restoration, and shutdown in a disposable Wine test process. Native Windows smoke and concurrent reads are being tested separately. This is not yet proven inside real Bottles games or as a production-safe attachment mechanism; INT-1102/1103/1104 remain pending. **Progress 16% (previous 16%, delta +0 percentage points)**.

**First live Wine process attachment proof (2026-11-02):** [run 41371726886](https://github.com/sguzman/intimatr/actions/runs/41371726886), `wine-load-smoke` job, confirmed the Linux-built PE helper loaded `intimatr.dll` into an already-running owned Wine fixture, with target-observed Running/Stopped lifecycle. INT-1003 / INT-1407 are still open: native Bottles manager selection and real-game verification are absent. **16% verified, delta +0 pp**.

**Linux-native running-process speed control proof (2026-11-02):** [CI 41373127184](https://github.com/sguzman/intimatr/actions/runs/41373127184) succeeded with the native `intimatr-bottles probe-attach` CLI loading a Windows DLL into an already-running Wine test process, then the target measuring all four timers at 0.5×/2×/1× and restoring its imports. **Real-game/Bottles validation and production-safe attachment are not verified; 16% expansion, previous 16%, delta +0 percentage points.**
