# Intimatr Project Specification

Intimatr is a Rust-first, in-process memory research toolkit intended for offline single-player games. It is meant to feel like an embedded and programmable subset of Cheat Engine: a DLL loaded into the target process, a CE-style scanner, debugger-oriented UI, and a local RPC surface for external frontends and automation.

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
    │   ├── scan/watch state
    │   └── frontend-neutral results
    ├── debugger
    │   ├── threads/registers
    │   ├── disassembly
    │   └── breakpoints
    ├── in-process GUI
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
- Keep Windows-specific unsafe code behind small, auditable modules; the scanner predicate and protocol layers should remain platform-neutral and testable.

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
- debugger behavior
- UI behavior
- policy gates for read/write/patch/debugger/remote-control capabilities

## Frontend contract

The in-process UI and RPC server both call the shared `CommandDispatcher`. Memory operations, scan sessions, watches, policy checks, transfer limits, and future debugger commands must be implemented once behind that boundary rather than independently in each frontend.

The RPC protocol is explicitly versioned. Version 1 uses four-byte big-endian length-prefixed JSON messages with request IDs. TCP is restricted to loopback addresses. The Windows named-pipe transport rejects remote clients. Long command execution is kept off the RPC I/O reactor so separate clients remain responsive enough to issue scan cancellation requests.
