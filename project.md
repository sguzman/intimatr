# Intimatr Project Specification

Intimatr is a Rust-first, in-process memory research toolkit intended for offline single-player games. It is meant to feel like an embedded and programmable subset of Cheat Engine: a DLL loaded into the target process, a CE-style scanner and native tool UI, debugger-oriented tooling, and a local RPC surface for external frontends and automation.

## Scope

The target architecture is:

```text
Game.exe
└── intimatr.dll
    ├── bootstrap/lifecycle
    ├── configuration + logging
    ├── memory engine
    │   ├── region enumeration
    │   ├── typed reads/writes
    │   ├── first scan / next scan
    │   └── result snapshots
    ├── shared command dispatcher
    │   ├── policy enforcement
    │   ├── scan/watch/freeze state
    │   ├── module/thread inspection
    │   ├── debugger state/events
    │   └── frontend-neutral results
    ├── native CE-style UI
    │   ├── scanner/results
    │   ├── watches/freezes
    │   ├── memory viewer/editor
    │   └── module/thread browser
    ├── debugger
    │   ├── thread contexts/registers
    │   ├── x86/x64 disassembly
    │   ├── per-thread pause/resume/step
    │   ├── DR0–DR3 hardware breakpoints
    │   ├── scoped VEH event capture
    │   └── native debugger UI
    └── local RPC server
         ├── loopback TCP
         ├── Windows named pipe
         └── arbitrary external frontends
```

The project is for offline/single-player research and does not include anti-cheat bypass, stealth, protection evasion, or multiplayer tooling.

## Development rules

- Rust is the implementation language for the DLL and first-party tooling.
- New code gets tests where practical.
- Run `cargo fmt`, `cargo clippy`, and `cargo test` before considering work complete.
- Keep dependencies current and prefer the latest stable release when introducing a crate.
- Use structured `tracing` instrumentation throughout subsystems and important state transitions.
- Keep configuration, policy gates, tuning knobs, and runtime parameters in TOML rather than hard-coding them.
- There is exactly one TOML configuration file per target game executable.
- Config files are named `<ExecutableName>.toml`, including the `.exe` suffix; for example, `ExampleGame.exe.toml`.
- Update `MILESTONES.md` as work lands and check completed items off in the same change.
- Keep Windows-specific unsafe code behind small, auditable modules; the scanner predicate, disassembler DTOs, command protocol, and RPC layers should remain platform-neutral and testable.

## Scanner semantics

The scan engine is general-purpose rather than game-specific. First-scan and next-scan filtering supports:

- unknown initial value
- exact / not equal
- greater than / greater-or-equal
- less than / less-or-equal
- inclusive range
- changed / unchanged
- increased / decreased
- increased by / decreased by

Historical predicates operate against a previous scan snapshot. Float equality is controlled by a per-game `scanner.float_epsilon` setting.

The scanner is intentionally split from Windows process access. `MemorySource` supplies normalized memory regions and exact byte reads. This lets the same scanner run against deterministic synthetic buffers in tests and `CurrentProcessMemory` inside the loaded DLL.

Supported typed values are `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, and `f64`. Scan candidates store the current scalar plus the immediately previous scalar after a next scan.

## Memory engine

On Windows, `CurrentProcessMemory` enumerates regions with `VirtualQuery` and uses `ReadProcessMemory`/`WriteProcessMemory` against the current-process pseudo-handle. Region metadata records committed/readable/writable/executable/guard state.

Writes are denied unless `policy.allow_memory_write` is enabled. Writes into executable regions additionally require `policy.allow_code_patch`. Non-writable committed regions may be temporarily changed to a writable protection for the duration of an allowed write, after which the original protection is restored. Executable writes flush the instruction cache.

## Configuration

Configuration is resolved from the current executable filename. If the target is `SomeGame.exe`, Intimatr loads `config/SomeGame.exe.toml` and validates that `[target].executable` matches the actual executable name case-insensitively.

The configuration owns:

- target identity
- logging settings
- RPC transport, endpoint, client/frame/transfer/result-page limits
- scanner tuning parameters, access requirements, result limits, alignment, and float epsilon
- debugger enablement, disassembly limits, breakpoint/event limits, native debugger UI, hotkey, and polling cadence
- general CE-style UI enablement, initial visibility, topmost mode, hotkey, dimensions, watch refresh cadence, and scan paging
- policy gates for read/write/patch/debugger/remote-control capabilities

## Frontend contract

The in-process UIs and RPC server all call the same `CommandExecutor`/`CommandDispatcher` instance created during bootstrap. Memory operations, scan sessions, watches/freezes, module/thread enumeration, policy checks, transfer limits, and debugger commands must be implemented once behind that boundary rather than independently in each frontend.

A frozen watch stores its target scalar in shared watch state. `RefreshWatches` reapplies that value through the normal policy-gated write path before reading it back. The first-party UI drives refreshes at its configured cadence, while RPC clients see and manipulate the same watch definition rather than a second frontend-specific freeze list.

The RPC protocol is explicitly versioned. Version 1 uses four-byte big-endian length-prefixed JSON messages with request IDs. TCP is restricted to loopback addresses. The Windows named-pipe transport rejects remote clients. Long command execution is kept off the RPC I/O reactor so separate clients remain responsive enough to issue scan cancellation requests.

Debugger events are shared state as well. The Windows debugger backend records Intimatr-owned events into a bounded sequence-numbered ring. `DebuggerEvents { after_sequence, limit }` exposes a cursor feed through the command layer; the native debugger UI polls that command and RPC clients can consume the same ordered stream without a second event system.

## Native UI contract

The general CE-style UI uses eframe/egui with the Glow backend as a normal native Windows tool window owned by the loaded DLL. The event loop is created on Intimatr's dedicated post-bootstrap UI thread; `DllMain` remains limited to minimal loader-safe work.

The UI dispatches commands through worker threads and receives command results back through a local response channel, keeping scans and memory operations off the render/event-loop thread. Window visibility is controlled by a configurable Windows virtual-key hotkey. Closing the window hides it; runtime shutdown closes and joins the UI thread. eframe persistence is scoped to a directory under `ui/<ExecutableName>` beside the DLL so UI/window state remains target-specific.

The native UI does not depend on hooking the target renderer. Renderer interception or an overlaid game-surface UI is not required for the CE-style tool-window workflow.

## Debugger contract

The debugger is in-process and deliberately narrower than a conventional external debugger. It owns a dedicated native tool window and a `DebuggerCore`, but all externally visible behavior remains behind the shared command layer.

Thread control is per selected thread. Intimatr records only suspensions that it created. If a thread already has a non-zero suspend count when Intimatr attempts to take ownership, the operation is rolled back and rejected instead of guessing who owns the existing suspension. The command worker's current thread is never suspended or context-mutated by the debugger.

Register snapshots use Windows thread-context APIs on a selected non-current thread. x64 context storage is wrapped in explicit 16-byte alignment before `GetThreadContext`/`SetThreadContext` calls. Disassembly is platform-neutral above the memory-read boundary and uses `iced-x86` for 16/32/64-bit decoding and Intel formatting.

Hardware breakpoints use the architecture's DR0–DR3 address slots per thread. Execute breakpoints use size 1; data breakpoints support the architecture encodings for 1/2/4/8-byte aligned ranges. These breakpoints do not patch game code.

A vectored exception handler is acquired only when stepping or hardware breakpoints require it. It consumes only `EXCEPTION_SINGLE_STEP` events that match Intimatr's own registered per-thread breakpoint/single-step state. Unrelated single-step exceptions return `EXCEPTION_CONTINUE_SEARCH`. Recorded breakpoint hits are trace-style events that auto-continue after Intimatr cleans the relevant debug state; there is no process-wide stop-the-world claim in this milestone.

Shutdown removes known hardware breakpoints, resumes threads still owned by Intimatr, cancels active scans through the shared executor, stops the UIs/RPC server, and releases the scoped exception handler outside loader-lock work.
