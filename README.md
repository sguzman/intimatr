# Intimatr

Intimatr is a Rust-first embedded memory research toolkit for offline single-player games. The long-term design is a DLL that hosts a Cheat Engine-like scanner, debugger-oriented UI, and local RPC server while sharing one core implementation between every frontend.

The repository is currently at the foundation milestone: configuration, logging, scan predicate semantics, tests, CI, and DLL crate output are defined. Real Windows memory traversal, the RPC server, GUI, and debugger are the next milestones.

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

At runtime, `ExampleGame.exe` will resolve to `config/ExampleGame.exe.toml`. The `[target].executable` value is then validated against the actual process executable name.

`config/ExampleGame.exe.toml` is the initial reference configuration and contains logging, RPC, scanner tuning, debugger, UI, and policy sections.

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
