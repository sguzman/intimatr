# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. The long-term design is a DLL that hosts a Cheat Engine-like scanner, debugger-oriented UI, and local RPC server while sharing one core implementation between every frontend.

The Windows DLL bootstrap/lifecycle milestone is complete: `intimatr.dll` now has a minimal loader-lock-aware entrypoint, deferred bootstrap thread, per-game config resolution, structured logging initialization, panic-contained FFI boundaries, explicit lifecycle states, and an exported clean-shutdown entrypoint. Real Windows memory traversal and scan sessions are the next milestone.

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

## Runtime layout

Per-game configuration is resolved relative to the loaded DLL, not the game's current working directory. A typical deployment is:

```text
Intimatr/
├── intimatr.dll
├── config/
│   └── ExampleGame.exe.toml
└── logs/
    └── intimatr.log
```

When loaded into `ExampleGame.exe`, the bootstrap thread resolves `config/ExampleGame.exe.toml`, validates `[target].executable`, anchors relative runtime paths such as `logging.directory` to the DLL directory, initializes tracing, and transitions the runtime to `Running`.

The exported `intimatr_lifecycle_state` function returns the numeric lifecycle state. `intimatr_request_shutdown` performs the full shutdown path outside `DllMain`; loaders that intentionally unload the DLL should call it before unloading so the non-blocking logging worker can flush and stop cleanly.

## Per-game configuration

Each executable gets exactly one TOML file named after the executable, including the `.exe` suffix:

```text
config/
└── ExampleGame.exe.toml
```

`config/ExampleGame.exe.toml` is the reference configuration and contains logging, RPC, scanner tuning, debugger, UI, and policy sections.

## Scanner predicate core

The platform-neutral predicate engine already models the filtering semantics needed for CE-style first/next scans:

- exact / not equal
- `>` / `>=`
- `<` / `<=`
- inclusive range
- changed / unchanged
- increased / decreased
- increased by / decreased by

Historical predicates require a previous value snapshot. Float equality uses the per-game `scanner.float_epsilon` value.

See `project.md` for the architectural contract and `MILESTONES.md` for ordered implementation work.
