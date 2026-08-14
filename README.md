# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. The long-term design is a DLL that hosts a Cheat Engine-like scanner, debugger-oriented UI, and local RPC server while sharing one core implementation between every frontend.

The Windows DLL lifecycle and first memory/scanner engine are now implemented. The DLL can resolve its per-game configuration, enumerate the current process memory map, perform typed reads and policy-gated writes, and run CE-style first/next scans. The shared command/RPC layer is the next milestone.

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

Scanner configuration includes chunk size, alignment, maximum result count, float epsilon, readable/writable/executable access requirements, and guard-page inclusion. Policy configuration separately controls memory writes and executable-code patches.

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

See `project.md` for the architectural contract and `MILESTONES.md` for ordered implementation work.
