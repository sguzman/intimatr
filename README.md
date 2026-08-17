# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. It is built as a Windows DLL with one shared command core behind a Cheat Engine-style scanner, native CE-style tool windows, an in-process debugger, advanced memory-analysis primitives, bounded runtime execution, and local RPC for external frontends.

Milestones 0–9 are implemented. The DLL resolves per-game configuration, enumerates current-process memory, performs typed reads and policy-gated writes, runs scalar and wildcard byte-pattern searches, maintains shared scans/watches, resolves pointer chains and module-relative addresses, inspects structures, exposes debugger state/events, shares the same command state across native UIs and RPC clients, applies bounded command backpressure, and ships through verified Windows x86_64 CI/release packaging.

## Build

On Windows with a current stable Rust toolchain:

```powershell
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release --lib --target x86_64-pc-windows-msvc
```

The verified CI DLL path is:

```text
target\x86_64-pc-windows-msvc\release\intimatr.dll
```

Large scanner benchmarks are available with:

```powershell
cargo bench --bench scanner
```

See `docs/performance.md` for benchmark shape and interpretation.

## Per-game configuration

Each executable gets exactly one TOML file named after the executable, including the `.exe` suffix:

```text
config/
└── ExampleGame.exe.toml
```

At runtime, `ExampleGame.exe` resolves to `config/ExampleGame.exe.toml` relative to `intimatr.dll`. The `[target].executable` value is validated against the actual process executable name.

Scanner configuration includes chunk size, alignment, maximum result count, float epsilon, readable/writable/executable access requirements, and guard-page inclusion. Policy configuration separately controls memory writes, executable-code patches, debugger access, and remote shutdown.

The `[runtime]` section controls the shared bounded command pool (`command_workers` and `command_queue_capacity`). The `[ui]` section controls the scanner/memory tool window. The `[debugger]` section controls debugger enablement, disassembly/event limits, hardware-breakpoint limits, debugger-window behavior, its hotkey, and event polling cadence. The sample configuration uses `Insert` for the CE-style tool window and `F10` for the debugger window.

## CE-style native UI

The first-party UI is an eframe/egui native Windows tool window created from a dedicated post-bootstrap thread. No GUI/event-loop work is performed from `DllMain`, and the UI does not need to hook the target game's DirectX/Vulkan/OpenGL renderer.

The UI provides:

- CE-style first/next scan controls for all typed scanner predicates
- paged scan results with one-click watch creation
- shared watch values plus freeze/unfreeze behavior
- raw memory reads, a 16-byte hex/ASCII view, and policy-gated hex writes
- loaded-module browsing
- current-process thread browsing
- TOML-driven visibility hotkey and initial window behavior
- per-target persisted window/UI state

Closing the tool window hides it instead of unloading Intimatr; use the configured hotkey to show it again.

See `docs/ui.md` for the UI configuration and runtime model.

## Debugger core and UI

The debugger is a second native eframe/egui tool window backed by the same shared command service used by RPC. It does not attach an external Windows debugger or patch the target renderer.

The debugger provides:

- process-thread selection and x64 register/context snapshots
- per-thread pause/resume ownership plus one-instruction trap-flag stepping
- x86/x64 disassembly through `iced-x86`
- RIP-to-disassembly navigation and disassembly-to-breakpoint navigation
- per-thread DR0–DR3 hardware execute/write/read-write breakpoints
- a narrowly scoped vectored exception handler for Intimatr-owned hardware-breakpoint and single-step exceptions
- an ordered fixed-capacity debugger-event feed consumed by both the debugger UI and RPC
- target-specific persisted debugger-window state

Thread suspension is deliberately local and explicit: Intimatr tracks only suspensions that it created and refuses ambiguous pre-existing suspend counts rather than pretending to own them. Hardware breakpoint hits are trace-style events and automatically continue after Intimatr records/cleans its own debug state; the debugger does not claim a global stop-the-world external-debugger model.

See `docs/debugger.md` for the debugger model and command surface.

## Memory and scanner core

`CurrentProcessMemory` is the Windows in-process backend. It normalizes `VirtualQuery` results into committed/readable/writable/executable/guard metadata and implements exact byte reads/writes.

The platform-neutral scalar scanner supports unknown-initial-value scans, exact/comparison/range predicates, changed/unchanged, increased/decreased, and delta predicates for signed integers, unsigned integers, `f32`, and `f64`. First scans traverse eligible regions in configured chunks and alignment; next scans refresh existing candidates against their previous snapshots.

Scans expose cancellation, progress callbacks, result limits, read-failure statistics, elapsed time, and throughput logging. Milestone 7 adds reproducible Criterion large-scan benchmarks and reuses scratch/result allocations across scalar/AOB chunk traversal rather than allocating a fresh byte vector for every chunk.

## Advanced search and analysis

The advanced-analysis layer adds reusable primitives without creating a second memory-access or policy layer:

- array-of-bytes scanning with exact bytes plus full/nibble wildcards such as `??`, `?F`, and `A?`
- absolute and case-insensitive module-relative expressions such as `Game.exe+0x1234`
- explicit 32/64-bit pointer-chain resolution and bounded reverse pointer-chain search
- structure-oriented scalar, pointer, and raw-byte inspection
- named saved scalar scan sessions and reusable watch templates
- per-target, versioned JSON analysis workspaces
- sequential `analysis.batch` automation through the same command/RPC implementation

Saved watch templates prefer module-relative addresses when the address belongs to a loaded module, making them naturally ASLR-friendly. Saved scan sessions remain snapshots of their original candidate addresses and should be revalidated after process-layout changes.

See `docs/analysis.md` for pattern syntax, pointer semantics, address expressions, persistence, and RPC examples.

## Shared commands, backpressure, and RPC

The `command` module is the frontend-neutral service boundary for memory operations, scan sessions, watches/freezes, module/thread enumeration, debugger operations, advanced analysis, and lifecycle. Policy and transfer limits are enforced there, so native UIs and RPC clients cannot accidentally implement different access behavior or maintain separate state.

At runtime the dispatcher is wrapped by one fixed-size bounded command executor shared by both native UIs and RPC. The number of active workers and queued commands is configured per executable. When producers outpace that queue, submission applies backpressure rather than manufacturing unbounded command work inside the game process. RPC separately bounds concurrent clients and frame sizes, while debugger events live in a fixed-capacity sequence-numbered ring.

RPC protocol v1 uses length-prefixed JSON and supports localhost TCP plus Windows named pipes. TCP binds are restricted to loopback addresses, named pipes reject remote clients, and request/response sizes and concurrent clients are bounded by each game's TOML file. Debugger events are exposed as a sequence-number cursor feed, while advanced analysis is exposed through the serialized `analysis` command and nested `AnalysisCommand` payloads.

A first-party client example is included:

```powershell
cargo run --example rpc_client -- 127.0.0.1:31337
```

See `docs/rpc.md` for framing and transport details.

## Shutdown and lifecycle hardening

`DllMain` remains minimal. Runtime shutdown owns subsystem ordering outside loader lock: it stops new RPC requests, closes/joins debugger and general UI threads, shuts down/cancels the shared command/debugger core, records the stopped lifecycle state, and explicitly drains the non-blocking tracing worker.

An abrupt process termination can always preempt user-space cleanup; the project therefore treats this as orderly/crash hardening rather than promising persistence after an operating-system kill.

## CI and releases

Normal CI runs formatting, strict Clippy, tests, an explicit `x86_64-pc-windows-msvc` release DLL build, verifies that `intimatr.dll` exists and is non-empty, and uploads it as an Actions artifact.

The release workflow can be run manually to validate/package a build. Tags of the form `v<package-version>` additionally publish a GitHub release; the workflow rejects tags that do not match `Cargo.toml`. Release ZIPs contain the DLL, optional PDB symbols, example TOML, project/README documentation, protocol/version notes, analysis/debugger/UI docs, performance notes, and extension invariants.

See `docs/versioning.md` for compatibility surfaces and `docs/extensions.md` for the frontend/plugin contract and subsystem invariants.

`project.md` contains the full architectural contract and `MILESTONES.md` records ordered implementation work.
