# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. The long-term design is a DLL that hosts a Cheat Engine-like scanner, debugger-oriented UI, and local RPC server while sharing one core implementation between every frontend.

The Windows DLL lifecycle, memory/scanner engine, shared command dispatcher, and local RPC transport are implemented. The DLL resolves per-game configuration, enumerates current-process memory, performs typed reads and policy-gated writes, runs CE-style first/next scans, and exposes the same command core to external Rust or other-language frontends over framed local RPC.

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

The `command` module is the frontend-neutral service boundary for memory operations, scan sessions, watches, lifecycle operations, and future module/debugger commands. Policy and transfer limits are enforced there, so an in-process UI and an RPC client cannot accidentally implement different safety or access behavior.

RPC protocol v1 uses length-prefixed JSON and supports localhost TCP plus Windows named pipes. TCP binds are restricted to loopback addresses, named pipes reject remote clients, and request/response sizes and concurrent clients are bounded by each game's TOML file.

A first-party client example is included:

```powershell
cargo run --example rpc_client -- 127.0.0.1:31337
```

See `docs/rpc.md` for framing and transport details, `project.md` for the architectural contract, and `MILESTONES.md` for ordered implementation work.
