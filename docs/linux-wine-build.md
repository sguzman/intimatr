# Linux-first builds and Wine process loading

Intimatr is authored and **built on Linux**, while its injected component remains a **Windows PE DLL**. A Linux host does not load `intimatr.dll` with `dlopen`: a target Windows program running under Wine must load it through a validated Windows/Wine path. The future Bottles-aware deployment/attach manager will run **natively on Linux** and select the correct Wine prefix, target architecture, and executable. This is the primary product path, not an optional cross-platform afterthought.

## Build on EndeavourOS / Arch Linux

Prerequisites: an available Rust toolchain with `rustup` and the MinGW-w64 GCC linker. The repository's `.cargo/config.toml` supplies the `x86_64-w64-mingw32-gcc` linker for `x86_64-pc-windows-gnu`.

```fish
yay -S --needed mingw-w64-gcc
rustup target add x86_64-pc-windows-gnu

cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release --lib --target x86_64-pc-windows-gnu

file target/x86_64-pc-windows-gnu/release/intimatr.dll
```

Expected output path: `target/x86_64-pc-windows-gnu/release/intimatr.dll`, a **PE32+ x86-64 DLL**, not a Linux ELF shared object. This path is a build goal; use a passing CI run or the local build output to verify an artifact actually exists. For an alternative Rust installation managed outside rustup, verify that the `x86_64-pc-windows-gnu` target standard library is available before building.

The original Windows MSVC build remains a supported regression target and is not removed. The new Linux CI job performs native core format/Clippy/tests, cross-builds the Windows GNU DLL, verifies its PE type and publishes a CI artifact. It must pass before we claim the Linux build is verified. GNU-built DLLs may have runtime dependencies that differ from MSVC builds: check actual imports and packaging in later compatibility work, do not assume them away.

## Read-only native Bottles inventory (first CLI slice)

A native Rust executable is available at `src/bin/intimatr-bottles.rs`. After the host Rust build, this **does not install or attach a DLL**:

```fish
cargo run --locked --bin intimatr-bottles -- scan
```

It invokes the installed non-Flatpak `bottles-cli --json list bottles`, then `bottles-cli --json programs -b <bottle-name>` for each reported bottle, with no shell interpolation and no game modifications. The result is a JSON array of bottle names and **registered** program metadata. Manual/unregistered EXEs are not silently claimed as discovered. Empty and malformed lists are handled as distinct states; CLI errors are surfaced rather than being treated as empty libraries.

The portable decoding logic is in [src/bottles.rs](../src/bottles.rs), with tests based on Bottles' published program JSON examples and synthetic list shapes. The Bottles documentation has not established a single exhaustive list-bottles JSON schema. **A scan on a real native Bottles installation remains an acceptance gate for INT-1401**, including validation of the names and paths returned. CI can prove parsing and compile a Linux executable; it cannot substitute for the end-user's Bottles process inventory.

## Loading/attaching from a Linux host

The deployment CLI is a **Linux native Rust application**; the payload is the cross-compiled Windows DLL. The project's loader must offer two distinct user-visible paths where supported:

1. **Managed launch-time load:** choose explicit Bottles games, preview modifications, and arrange an actual DLL load in their normal Wine startup path; report an observed loaded module and functioning clock hooks. A native/builtin override is a load-order preference, **not** an instruction to load an arbitrary DLL.
2. **Attach to an already-running Wine process:** identify a specific owned target inside its bottle, verify executable/architecture/prefix and permission, then use a tested Wine-compatible mechanism to load the Windows DLL without replacing the Wine runner. If that Wine/runner combination cannot support the operation safely, report unsupported rather than claiming attachment. Avoid forceful OS-level ptrace or protection evasion as a default strategy.

Both paths require repeatable tests of actual DLL load, startup, shutdown, game timing, error recovery and optional detachment/cleanup semantics. The Linux manager must never silently target an arbitrary Wine process or alter all bottles by default. Protected, anti-cheat-enabled and multiplayer targets are excluded.

The existing Intimatr `DllMain` only bootstraps when loaded; **it is not itself an injector**. Likewise, the current per-exe config resides beside the DLL and has not yet been made global. Both features need new implementation and tests. Do not deploy the incomplete DLL to an entire library.

## Live-process attachment: proposed testable mechanism

An ordinary Linux process cannot use a Wine Windows-process handle as if it were a native Linux PID. The native Rust manager should identify the **bottle, prefix and Windows executable**, then launch a small **cross-compiled PE helper** under that same Wine prefix/runner. That helper can use Windows process APIs to enumerate/open the opted-in process and request a normal Windows `LoadLibraryW` load. This is a *design candidate*, not a verified implementation.

The helper must operate only on a uniquely identified process chosen by the user and report how it matched the target (executable identity, prefix identity, bitness, process handle). It should validate the DLL's PE architecture before loading, avoid assuming that function addresses are identical across processes, wait for a positive module/Intimatr-ready handshake, return structured errors, and support a no-mutation plan mode. It should not deploy stealth, manual mapping, APC evasion, anti-cheat bypass or general-purpose attach to unrelated processes.

**Why not simply use WINEDLLOVERRIDES?** Wine's own documentation describes this as choosing native/builtin *load order for an already requested DLL*. It is not proof that a game loads Intimatr. Game-local proxy DLLs can be a fallback only if they do not conflict with existing files and a clear rollback is possible. A direct Wine-compatible PE loader is a better candidate for attaching after the game starts.

**Compatibility caution:** historical Wine implementations have differed in cross-bitness remote-thread support, especially legacy WoW64 mode. Do not treat same-architecture test success as proof that a 64-bit helper can attach to a 32-bit target. Separate 32-/64-bit helpers or explicit unsupported reporting are required.

**Proof-of-attach acceptance:** build PE helper on Linux; run under a disposable or test bottle; identify an offline Windows test process already running in that prefix; load an innocuous Intimatr test DLL; verify module presence and exported readiness state; repeat/no-op duplicate attach; reject wrong-bottle, wrong-architecture, protected and ambiguous targets; then validate normal load/cleanup in a real selected offline game. Until this passes, the CLI must not report a game as attached.

Related technical references (research, not imported project code): [proton-injector implementation and documentation](https://github.com/jokelbaf/proton-injector), [Wine API-side loader example in decant](https://github.com/RobbyV2/decant/blob/main/docs/ARCHITECTURE.md), [historical Wine cross-bitness remote-thread limitation](https://list.winehq.org/hyperkitty/list/wine-bugs%40list.winehq.org/thread/ROQNGR7JMYKWAIEH3TKKAKZKRLFG22KX/).

## Bottles discovery and config notes

Bottles exposes `bottles-cli` with `list`, `programs`, `edit`, `run` and other subcommands; a native (non-Flatpak) install should use the installed CLI command directly, with capability detection rather than assuming an installation path. Registered program inventory alone may not find a manually launched EXE; the future scanner will distinguish verified registered games from inferred candidates.

The proposed profile order is built-in safe defaults, shared global speed configuration, per-bottle selection/overrides, per-executable overrides, and ephemeral session speed. The implementation must define a robust Linux-to-Wine config path mapping without overwriting Intimatr's existing per-target scanner/debugger config files.

## Current validation boundaries

- The versioned roadmap is in [speed-control-roadmap.md](speed-control-roadmap.md); milestone 10 covers feasibility, 11 the clock core, 14 actual managed loading/attach, 16 Wine compatibility, and 17 distribution.
- The platform-neutral clock core is implemented as `src/speed.rs` with synthetic time-transition tests. **It does not hook a timer, load a DLL, or alter an existing Wine process.**
- Linux CI and Wine runtime status must be checked on the latest commit before asserting readiness. The current repository does not yet ship a working Linux Bottles deployment manager or a verified live-process attacher.

References: [ArchWiki Rust cross-compilation](https://wiki.archlinux.org/title/rust), [Bottles CLI](https://docs.usebottles.com/advanced/cli), [Wine DLL override behavior](https://manpages.debian.org/unstable/libwine/wine64.1.en.html).
