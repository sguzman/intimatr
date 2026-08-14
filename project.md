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
    ├── debugger
    │   ├── threads/registers
    │   ├── disassembly
    │   └── breakpoints
    ├── in-process GUI
    └── RPC command server
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

The scan engine is general-purpose rather than game-specific. At minimum, first-scan and next-scan filtering must support:

- exact / not equal
- greater than / greater-or-equal
- less than / less-or-equal
- inclusive range
- changed / unchanged
- increased / decreased
- increased by / decreased by

Historical predicates operate against a previous scan snapshot. Float equality is controlled by a per-game `scanner.float_epsilon` setting.

## Configuration

Configuration is resolved from the current executable filename. If the target is `SomeGame.exe`, Intimatr loads `config/SomeGame.exe.toml` and validates that `[target].executable` matches the actual executable name case-insensitively.

The configuration owns:

- target identity
- logging settings
- RPC transport/bind limits
- scanner tuning parameters
- debugger behavior
- UI behavior
- policy gates for read/write/patch/debugger/remote-control capabilities

## Frontend contract

The eventual UI and RPC server should both call a shared command/service layer. Memory scanning, debugger operations, and policy enforcement must not be duplicated independently in each frontend.
