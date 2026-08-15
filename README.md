# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. It is built as a Windows DLL with one shared command core behind a Cheat Engine-style scanner, native CE-style tool windows, an in-process debugger, and local RPC for external frontends.

Milestones 0–4 are complete, and the Milestone 5 debugger implementation is on the current development branch. The DLL resolves per-game configuration, enumerates current-process memory, performs typed reads and policy-gated writes, runs first/next scans, maintains shared watches/freezes, enumerates loaded modules and process threads, and exposes the same command state to the native UIs and RPC clients.

## Build

On Windows with a current stable Rust toolchain:

```powershell
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release --lib
```

The DLL will be produced at:

```text
target\release\intimatr.dll
```

## Per-game configuration

Each executable gets exactly one TOML file named after the executable, including the `.exe` suffix:

```text
config/
└── ExampleGame.exe.toml
```

At runtime, `ExampleGame.exe` resolves to `config/ExampleGame.exe.toml` relative to `intimatr.dll`. The `[target].executable` value is validated against the actual process executable name.

Scanner configuration includes chunk size, alignment, maximum result count, float epsilon, readable/writable/executable access requirements, and guard-page inclusion. Policy configuration separately controls memory writes, executable-code patches, debugger access, and remote shutdown.

The `[ui]` section controls the scanner/memory tool window. The `[debugger]` section controls debugger enablement, disassembly/event limits, hardware-breakpoint limits, debugger-window behavior, its hotkey, and event polling cadence. The sample configuration uses `Insert` for the CE-style tool window and `F10` for the debugger window.

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

The debugger is a second native eframe/egui tool window backed by the same shared `CommandDispatcher` used by RPC. It does not attach an external Windows debugger or patch the target renderer.

The debugger provides:

- process-thread selection and x64 register/context snapshots
- per-thread pause/resume ownership plus one-instruction trap-flag stepping
- x86/x64 disassembly through `iced-x86`
- RIP-to-disassembly navigation and disassembly-to-breakpoint navigation
- per-thread DR0–DR3 hardware execute/write/read-write breakpoints
- a narrowly scoped vectored exception handler for Intimatr-owned hardware-breakpoint and single-step exceptions
- an ordered debugger-event feed consumed by both the debugger UI and RPC
- target-specific persisted debugger-window state

Thread suspension is deliberately local and explicit: Intimatr tracks only suspensions that it created and refuses ambiguous pre-existing suspend counts rather than pretending to own them. Hardware breakpoint hits are trace-style events and automatically continue after Intimatr records/cleans its own debug state; Milestone 5 does not implement a global stop-the-world external-debugger model.

The Windows x64 context wrapper explicitly provides the alignment required by the Windows context APIs before calling `GetThreadContext`/`SetThreadContext`.

See `docs/debugger.md` for the debugger model and command surface.

## Memory and scanner core

`CurrentProcessMemory` is the Windows in-process backend. It normalizes `VirtualQuery` results into committed/readable/writable/executable/guard metadata and implements exact byte reads/writes.

The platform-neutral scanner supports these predicates:

- unknown initial value
- exact / not equal
- `>` / `>=`
- `<` / `<=`
- inclusive range
- changed / unchanged
- increased / decreased
- increased by / decreased by

Supported scan types are `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, and `f64`. First scans traverse eligible regions in configured chunks and alignment. Next scans refresh only existing candidates and compare against their prior snapshots.

Scans expose cancellation, progress callbacks, result limits, read-failure statistics, elapsed time, and throughput logging. Deterministic synthetic-memory tests exercise chunk-boundary and historical-predicate behavior independently of Windows process memory.

## Shared commands and RPC

The `command` module is the frontend-neutral service boundary for memory operations, scan sessions, watches/freezes, module/thread enumeration, debugger operations, and lifecycle. Policy and transfer limits are enforced there, so the in-process UIs and RPC clients cannot accidentally implement different access behavior or maintain separate state.

RPC protocol v1 uses length-prefixed JSON and supports localhost TCP plus Windows named pipes. TCP binds are restricted to loopback addresses, named pipes reject remote clients, and request/response sizes and concurrent clients are bounded by each game's TOML file. Debugger events are exposed as a sequence-number cursor feed through the `debugger_events` command, so arbitrary clients can incrementally consume the same events shown by the native debugger UI.

A first-party client example is included:

```powershell
cargo run --example rpc_client -- 127.0.0.1:31337
```

See `docs/rpc.md` for framing and transport details, `project.md` for the architectural contract, and `MILESTONES.md` for ordered implementation work.
