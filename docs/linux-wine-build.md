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

## Preview selections without changing any game

The native Linux manager also supports a **read-only plan** for registered Bottles programs:

```fish
cargo run --locked --bin intimatr-bottles -- plan --include-bottle 'Games' --include-program '*.exe' --exclude-program '*Launcher*'
```

Other modes include `plan --all --exclude-bottle 'Online*'` or `plan --include-program 'Disco*.exe'`. Including both bottle and program filters narrows the match by intersection. Multiple patterns within one filter category are alternatives; exclusion always wins. Names and Windows paths use anchored ASCII-case-insensitive `*` and `?` globs with either slash style. The output distinguishes `selected`, `not_included`, `excluded_bottle` and `excluded_program`, and always declares its operation as `read_only_selection`.

`--all` is an explicit opt-in and cannot be combined with include rules; excludes alone do not implicitly select everything. Neither `scan` nor `plan` modifies game data, DLLs, environment settings, registry, launchers or running processes. The plan is a **candidate list**, not a claim that DLL loading will succeed. Real Bottles field verification is still needed.

## Isolated Wine DLL load smoke test

A minimal Windows-only test process, `src/bin/wine-load-probe.rs`, and a safe per-executable fixture, `config/wine-load-probe.exe.toml`, prove the **normal in-process LoadLibraryW path**, separate from injecting a third-party game. The test process loads the Linux-built `intimatr.dll`, resolves Intimatr's lifecycle exports, waits for Running, requests orderly shutdown, verifies Stopped, and unloads.

The CI `wine-load-smoke` job runs this in a disposable Ubuntu Wine prefix with UI, debugger, RPC and memory writes disabled; the original Windows CI, Linux GNU cross-build and native Bottles manager continue independently. A passing test proves Wine can load Intimatr in **one deliberately controlled test process**, not that the DLL loads automatically into all games or that timer hooks work. A failed test must be diagnosed, not claimed as a verified integration.

## 2026-11-02: Running-process attachment proven in disposable Wine fixture

[GitHub Actions run 41371726886](https://github.com/sguzman/intimatr/actions/runs/41371726886), job `wine-load-smoke`, **passed the remote attachment step**. Both PE executables ([target](../src/bin/wine-attach-target.rs) and [helper](../src/bin/wine-attach-helper.rs)) were built on Linux for `x86_64-pc-windows-gnu`. CI launched `wine-attach-target.exe` first in a disposable `WINEPREFIX`; the target wrote its Windows PID (32 in the observed run) before the helper was started.

The separately launched Windows helper, running under the **same Wine prefix**, checked that the requested PID belonged specifically to `wine-attach-target.exe`. It resolved `LoadLibraryW` to the actual containing system module, found that module's base in the target process and computed a target-relative start address rather than assuming identical absolute addresses. After writing the known `intimatr.dll` path to the target, it started and joined a remote loader thread. The target then independently confirmed the DLL reached Running and shut down to Stopped.

CI output explicitly showed:

```text
Offline Wine attach target awaiting DLL; pid=32
Wine helper completed remote LoadLibraryW thread for test PID 32
Wine attach target observed Running then Stopped
```

**Scope and important limitations:** this proves a *controlled* already-running Windows process can load the DLL through Wine on the tested Ubuntu x86-64 runner. The helper is hard-allowlisted to that exact fixture, does not enumerate or select personal games, and is not installed into Bottles. It is not a general-purpose injector, anti-cheat bypass, or a process-attach service. It does not prove compatibility with the user's native Bottles runner, cross-bitness, a real game, repeated attachment, timing hooks in the remote target, or safe live-game teardown. The run's independent strict lint jobs failed a helper style check subsequently corrected; the latest full CI result must be checked before broader verification claims.

**Next engineering gates:** connect the native Linux Bottles manager to targeted process/prefix discovery and an opt-in same-prefix Windows helper, add actual DLL readiness/status confirmation (not merely a remote thread return), verify correct process identity and compatibility before any application, and test duplicate attach/rollback. No automatic game-wide changes are authorized by this fixture.

## 2026-11-02 — Native Linux controller → remote Wine speed control

**Verified in [CI run 41373127184](https://github.com/sguzman/intimatr/actions/runs/41373127184), job `wine-load-smoke`.** On an isolated Ubuntu Linux host, the native `intimatr-bottles probe-attach` controller launched the cross-built Windows helper inside a disposable Wine prefix and attached `intimatr.dll` to an **already-running** `wine-attach-target.exe` (Windows PID 32 in this run). The target independently loaded the four timer hooks after Intimatr reached Running, measured rate changes against unmodified system timer addresses, restored its imports and shut down cleanly.

| Requested | QPC | GetTickCount64 | GetTickCount | timeGetTime |
| --- | ---: | ---: | ---: | ---: |
| 0.5× | 0.500× | 0.494× | 0.494× | 0.494× |
| 2× | 2.000× | 2.006× | 2.006× | 2.006× |
| 1× | 1.000× | 1.000× | 1.000× | 1.000× |

The CI log reported `"operation":"fixture_only_remote_load"` with `"game_attachment":false`, then the target printed `Wine attach target verified 0.5x/2x/1x timers, then Stopped`. The CLI tests require explicit PID/prefix/helper arguments, reject unknown or duplicate options, require a valid preinitialized prefix containing `drive_c` (not an eagerly written `system.reg` snapshot), and restrict the helper filename. The Windows helper additionally verifies the selected process image is `wine-attach-target.exe` and refuses other names.

**This is not a production game-injection feature.** No user Bottles library was read or changed. Real game/prefix identification, 32-bit support, opt-in match filters for live Windows processes, engine-specific timing APIs, environment compatibility with the user's patched Wine runner, anti-cheat exclusion, duplicate attachment, and full lifecycle teardown under a live game remain unverified. The test's `helper_exited_successfully` field means the helper returned successfully, **not** that the CLI independently interrogated and authenticated the target's live Intimatr status; the CI target supplies that separate confirmation.

Current verified scope remains limited to disposable x86-64 fixtures; these results support continuing INT-1003, INT-1407, INT-1102/1103 but do not close their full acceptance tests.

## 2026-11-02 — Target-side module confirmation and duplicate attach refusal

[CI run 41373871724](https://github.com/sguzman/intimatr/actions/runs/41373871724), `wine-load-smoke` job, passed a stricter **Linux native controller → same-prefix Windows helper → already-running Wine fixture** sequence:

1. The Windows helper checks for `intimatr.dll` in the target module list **before** allocating remote memory; it fails closed on an uninspectable module list, and refuses a second load when the module is already present.
2. After the remote `LoadLibraryW` thread finishes, it inspects the target module list again. A signaled remote thread alone no longer counts as successful DLL loading.
3. The fixture independently measures all four virtual clocks at 0.5×/2×/1×, then publishes a test-only readiness marker while its DLL stays loaded.
4. CI issues **the same Linux CLI attach command a second time** against the same still-live Windows PID, requires a nonzero exit and the specific `refusing duplicate attach` diagnostic, and only then releases the target for orderly shutdown.

Observed CI output confirmed the second attach was refused, after four timing APIs produced the expected rates, and the first target reached Stopped normally. This proves **sequential duplicate refusal**, not an atomic reservation for simultaneous competing helpers. It does not grant arbitrary Wine process access or automatic Bottles opt-in. The fixture executable allowlist remains unchanged and other program images remain refused.

These checks strengthen the INT-1003/INT-1407 attachment groundwork but do not satisfy either full acceptance criterion; **weighted expansion remains 16/100, previous 16, delta +0 percentage points**.
