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

The CI log reported `"operation":"fixture_only_remote_load"` with `"game_attachment":false`, then the target printed `Wine attach target verified 0.5x/2x/1x timers, then Stopped`. The CLI tests require explicit PID/prefix/helper arguments, reject unknown or duplicate options, require a valid preinitialized prefix containing `drive_c` (not an eagerly written `system.reg` snapshot), and restrict the helper filename. The Windows helper independently requires the target's **complete canonical executable path** to match the owned `wine-attach-target.exe` file beside the fixture helper. Other paths are refused even if their executable basename is identical.

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

## 2026-11-02 — Concurrent fixture attach exclusion

[GitHub Actions 41376199401](https://github.com/sguzman/intimatr/actions/runs/41376199401) passed all three CI jobs, including a **two-controller, same-PID race** in the isolated Wine prefix. Two native Linux `intimatr-bottles probe-attach` commands launched separate cross-compiled Windows helpers together; exactly one returned successful remote loading and the other exited with the expected `refusing duplicate attach` reason. A subsequent sequential attach was also rejected while the target remained active. The target independently measured all four timer API families at 0.5×/2×/1× and restored its imports before exiting.

The helper now uses a `Local\IntimatrFixtureAttach-<PID>` named Win32 mutex to serialize its preflight module check, remote load and positive module confirmation. It holds the lock via RAII until that operation finishes. Timeout and abandoned-lock states fail closed rather than treating inconsistent state as safe. This follows the documented Windows [named mutex](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexw) and [session namespace](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces) behavior.

**Limits:** this is a test-only mutex scoped to the same Wine Windows session and known PID. The helper remains allowlisted only for `wine-attach-target.exe`; it is **not** available for actual games. Different Wine sessions, crashes after a remote thread starts, process PID reuse, changes made outside this manager, real runner compatibility, authentic game identity and production cleanup still require separate engineering tests. In particular, a remote-load timeout can leave a loader thread in progress; production must track uncertain state durably rather than assuming the mutex alone solves it.

## 2026-11-02 — Linux-only read-only Wine prefix process discovery

The native Linux `intimatr-bottles processes --prefix INITIALIZED_WINEPREFIX` command now inspects **same-user Linux processes** in `/proc`. It compares the canonical absolute `WINEPREFIX` environment value to the specifically requested initialized prefix, and returns each matching process's **Linux PID** and observed Linux binary basename. It limits environment reads to 256 KiB, reports inaccessible/ambiguous environments, rejects relative or duplicated `WINEPREFIX` declarations, and does not print command arguments or other environment contents. Different prefixes are excluded; matching records are sorted by Linux PID.

**Crucial distinction:** inheriting `WINEPREFIX` proves only an environment association. It does **not** prove a process is a Wine game, that its executable matches a registered Bottles program, or that its Linux PID corresponds to a Wine/Windows PID. Therefore every result reports `windows_pid: null`, `windows_executable_verified: false`, and `safe_to_attach: false`. This command never opens a process for injection, attaches a DLL, or modifies a prefix. It is a preliminary, auditable input to future real game mapping, not permission to attach to a listed process.

A Linux CI fixture spawns a harmless same-user process with an isolated `WINEPREFIX`, verifies read-only discovery includes its Linux PID, and checks Windows PID remains explicitly unknown. Native Bottles runner and real-game process identity still require field verification.

## 2026-11-02 — Read-only Windows PID enumeration under the same prefix

The Windows PE helper [wine-process-list.rs](../src/bin/wine-process-list.rs) uses the Win32 ToolHelp **process snapshot API** to enumerate Windows PIDs and reported image names in its own Wine process namespace. Where permissions allow, it also queries the complete Windows image path. The Linux CLI runs that *read-only* helper explicitly under the selected initialized `WINEPREFIX`:

```fish
intimatr-bottles win-processes --prefix /path/to/your/prefix --helper /path/to/wine-process-list.exe --runner /absolute/path/to/wine
```

Output is bounded JSON with `windows_pid`, `reported_executable`, `full_image_path`, `image_query_verified`, and `safe_to_attach: false`. A missing full image path is reported as unknown, not reconstructed from the executable basename. The Linux CLI validates the helper's exact filename, process-list JSON shape and `attachment_attempted: false` marker before displaying it. The helper **does not inject, load Intimatr, or change processes**.

CI runs this helper in the **same Wine prefix as the already-running owned attachment fixture** and requires its process snapshot to contain that fixture's independently published Windows PID. This proves how to obtain Windows PID information without confusing it with `/proc` Linux PIDs. Comparing this inventory with registered Bottles executable paths is still a later task; no existing games are identified as safe to attach solely by this result.

## 2026-11-02 — Fail-closed registered-program/path correlation

The portable [bottles_live_matches.rs](../src/bottles_live_matches.rs) reconciles **one explicitly selected bottle**'s registered programs with a read-only Windows PID snapshot. It accepts candidates only when `QueryFullProcessImageNameW` succeeded and the complete drive-qualified Windows executable path exactly matches a registered Bottles program after case/slash normalization. A matching basename alone is ignored; duplicates in registered paths produce an ambiguity status rather than a selection. Unknown image paths, UNC/drive-relative aliases and unregistered executables remain in the report with reasons.

The Linux CLI adds a file-based, read-only `match-snapshot` subcommand consuming Bottles' `--json programs` array and the output of `win-processes`. It outputs candidate Windows PIDs and match statuses with **`safe_to_attach: false`** and `attachment_attempted: false` for *every* record; no permission to inject is inferred from path equality. Synthetic CI fixtures exercise same-basename programs in different paths and a Windows PID with an unreadable image path. This does not yet verify a real Bottles inventory or native game paths.

## Read-only dogfooding sequence (no process attachment)

Once a matching native Linux CLI and read-only Wine process-list helper have been built, the following sequence can inspect one **explicitly chosen initialized** prefix. Use the same Wine runner configured for that prefix; do not assume a patched runner's executable location.

```fish
intimatr-bottles processes --prefix /absolute/prefix/path
intimatr-bottles win-processes --prefix /absolute/prefix/path --helper /absolute/wine-process-list.exe --runner /absolute/path/to/wine
```

The first command lists same-user **Linux** PIDs with the requested environment prefix but leaves Windows PIDs unknown. The second starts an intentionally read-only Windows PE helper in that prefix and lists **Windows** PIDs, image names and queryable complete Windows paths. One does not establish a mapping to the other. To compare a saved Windows PID report with Bottles' registered programs in a *single selected bottle*, first save the native `bottles-cli --json programs -b 'Bottle Name'` JSON to a file, then run:

```fish
intimatr-bottles match-snapshot --bottle 'Bottle Name' --programs /absolute/programs.json --windows /absolute/windows-processes.json
```

The result is a **review-only candidate list** with every `safe_to_attach` value set to false. It deliberately cannot start the actual attachment helper, mutate Bottles configuration or operate on unrelated processes.

## 2026-11-02 — Native Bottles and Wine live-match (read-only)

The Linux controller now supports `intimatr-bottles live-match --bottle NAME --prefix WINEPREFIX --helper /path/to/wine-process-list.exe [--runner /path/to/wine] [--bottles-cli /path/to/bottles-cli]`. It queries the **native** `bottles-cli --json list bottles` and verifies the exact bottle name is present, reads the registered programs using `bottles-cli --json programs -b NAME`, then launches the already-tested read-only Windows PID helper in the **explicitly supplied** initialized Wine prefix. Finally it uses the existing exact full Windows executable-path matching logic and outputs a single JSON result. No files are written by the controller and no game DLL is loaded.

**Identity caveat:** Bottles' list and programs commands do not independently establish that the separately supplied `WINEPREFIX` belongs to the chosen bottle. The combined report therefore explicitly sets `bottle_prefix_association_verified: false`; even unique exact path matches always retain `safe_to_attach: false` and `attachment_attempted: false`. An unknown, missing or ambiguously matched game is never silently elevated to an attachable process. Only programs registered in Bottles are in scope; loose executables are not discovered automatically.

CI launches the controlled Wine target and real PE process helper, while replacing **only** `bottles-cli` with a small fixture implementing the documented JSON commands. It checks that the report includes the target Windows PID without falsely matching an unrelated registration, preserves the unknown bottle-prefix association, rejects a nonexistent bottle, and never grants attachment. Actual native Bottles field validation—including correctly resolving the real prefix and runner, symlinked game paths, and installed games—remains outstanding.

Bottles' documented syntax: https://docs.usebottles.com/advanced/cli

## 2026-11-02 — Native Bottles prefix ownership verification

Current source now uses the upstream native Bottles CLI's `info bottles-path` response to locate Bottles' **configured** bottles root, including customized installations. In upstream Bottles, this particular command prints a **plain directory path even when `--json` is supplied**. The JSON `list bottles` response can be a name-keyed object (and `{}` for an empty library), so the native inventory parser handles that schema as well as its older fixtures. The implementation follows Bottles' published [CLI documentation](https://docs.usebottles.com/advanced/cli) and upstream source, not a guessed home-directory location.

The read-only command can now infer the prefix directly:

```fish
intimatr-bottles live-match --bottle 'My Bottle' --helper /absolute/path/wine-process-list.exe --runner /absolute/path/to/wine
```

An optional `--prefix /absolute/path/to/prefix` is available for explicit verification. It must resolve to exactly the selected bottle directory under Bottles' configured root. Before starting the Windows probe, the controller validates that the bottle exists in native Bottles' list, its directory is a direct non-symlinked child of the configured root, and the prefix contains `bottle.yml` and `drive_c`. It rejects wrong prefixes, parent traversal, symlink aliases and missing markers. The result includes the canonical bottles root, resolved prefix and its source (`configured_bottles_root` or `explicit_verified`), plus a narrowly scoped `bottle_prefix_association_verified` indicator.

**Scope:** the indicator verifies a directory's association with Bottles' selected name at inspection time. It does **not** establish that a particular game process belongs to the bottle, that the supplied Wine runner is the one configured by Bottles, or that the target can be safely attached. Accordingly `runner_verified: false`, `game_process_identity_verified: false`, `safe_to_attach: false`, and `attachment_attempted: false` remain explicit. The read-only command doesn't load Intimatr into any game. Launching the Windows probe can still cause Wine's ordinary runtime metadata updates inside its prefix.

CI uses a disposable prefix named `Fixture`, a minimal `bottle.yml` marker and a synthetic `bottles-cli` reporting `info bottles-path`. It verifies automatic prefix resolution and refusal of a different explicit initialized prefix before querying Wine. Real native Bottles installations and custom runners have not been inspected, so INT-1401 and INT-1407 remain open.

## 2026-11-02 — Conservative Wine runner and PE file architecture evidence

The read-only `live-match` command now includes a `runner_audit` object derived from the **actual selected bottle's** metadata in the native `bottles-cli --json list bottles` response. When available, it reports `Runner`, `Arch`, `Environment` and `RunnerPath` without treating a matching string as proof of the executable used by the running game. It distinguishes system (`sys-*`), managed/external and Steam-style runners. Alternate name-only JSON inventory shapes remain supported, but report metadata as unknown. Invalid metadata types fail closed.

Bottles upstream resolves ordinary managed runners using its own global runner directory or external runner map; its `info bottles-path` response describes the **bottle storage root**, not the global runner root. Intimatr therefore **does not infer a runner path from the bottle directory**. A recorded Steam `RunnerPath` can be compared with an explicit runner executable, provided both paths resolve to the same existing executable file. Other cases remain `runner_verified: false`; even a matched recorded Steam runner does not authorize attachment. The runner command provided to `live-match` remains an explicit read-only probe choice.

A portable `pe_image` module now inspects Windows PE **file headers** using bounded reads: DOS `MZ` signature, PE signature, COFF machine and PE32/PE32+ optional-header magic. It recognizes x86, x86-64 and ARM64 pairs, and rejects malformed, truncated or contradictory signatures. The Windows `wine-process-list.exe` read-only process enumerator uses the *reported complete executable file path*, where readable, to report `pe_image_architecture` and `pe_image_verified`. These fields are preserved by `match-snapshot`/`live-match`, with inconsistent or forged validation flags rejected by the snapshot decoder.

**Important limits:** PE file architecture is not a trustable attestation of a mapped/running process, a guarantee that the file has not changed since launch, or proof the configured runner and selected program are the same. Unknown/unreadable images remain unverified, and every game stays `safe_to_attach: false` with `attachment_attempted: false`. Linux and Wine CI tests are exercising the new fields; a full live x86-64 image recognition claim requires their successful completion. Native Bottles/game field tests and production attach remain outstanding.

## 2026-11-02 — Live Wine process machine vs executable file architecture

The Windows read-only `wine-process-list.exe` helper now attempts a **live process machine query** using Win32 `IsWow64Process2` on each existing Windows PID. It resolves the function dynamically from `kernel32.dll` to remain compatible with Wine runners where the export is missing. It uses only a `PROCESS_QUERY_LIMITED_INFORMATION` process handle and does not request debugging, memory modification or remote execution. On success it interprets the `process_machine` value when nonzero (WOW64) or the `native_machine` value otherwise, accepting recognized x86, x86-64 and ARM64 machine constants. Unsupported, unknown or failed API calls produce **null/false**, not a guessed machine. Reference: https://learn.microsoft.com/en-us/windows/win32/api/wow64apiset/nf-wow64apiset-iswow64process2

Each PID record now separately exposes `process_architecture`, `process_architecture_verified`, `process_architecture_source`, `pe_image_architecture`, and `pe_image_verified`. The Linux snapshot parser validates their internal consistency and supports older fixture snapshots that lack the new optional fields. In the selected-bottle candidate report, `architecture_consistent` is `true` only when both observations exist and agree, `false` if they contradict, and `null` when evidence is missing. Any observed mismatch now changes the matching status to `process_file_architecture_conflict` and suppresses registered-game candidate selection **even if the executable paths match**.

The cross-Wine CI suite checks that its known x86-64 test target's **PE file** is x86-64 and, if the Wine runner successfully reports a live machine, that it too reports x86-64 with `is_wow64_process2` as source. If the API is unavailable, CI requires an explicit null/unverified result. This is a controlled fixture test, not proof against TOCTOU, binary replacements, copied names, filesystem aliases, or arbitrary native Bottles games.

The integration remains read-only and all candidates continue to report `safe_to_attach: false`, `attachment_attempted: false`. This live-machine query makes wrong-bitness detection less speculative but **does not** complete INT-1406 or authorize a general-purpose game attachment.

## 2026-11-02 — Explicit Wine runner selection and observed live architecture

Native Bottles can use a patched/custom Wine runner, while the host's `wine` command may point to a different installation. For that reason **both** native Linux read-only commands that launch a Windows PE probe now **require an explicit `--runner` argument**: `win-processes --prefix ... --helper ... --runner ...` and `live-match --bottle ... --helper ... --runner ...`. The prefix may still be derived automatically from Bottles' configured root. Without the runner argument these commands fail before starting Wine; they never silently select the host's default runner. The commands remain noninjecting, but a Wine process itself can perform ordinary prefix runtime writes, so this extra guard matters.

The Wine regression on [CI 41481506734](https://github.com/sguzman/intimatr/actions/runs/41481506734) independently observed the controlled already-running target's **live process architecture as `x86_64`** through `IsWow64Process2`, matching its PE-file architecture. The same Wine fixture verified duplicate-attach refusal, speed at 0.5×/2×/1× on four APIs, and orderly shutdown. This confirms the optional Win32 architecture query works in the tested Ubuntu Wine runner. It is not a claim that other Wine runners or a real Bottles game will behave identically.

Current acceptance gates still lacking: actual native Bottles runner path selection, cross-runner compatibility, real game identity, 32-bit support, production-safe hook lifetime and rollback. All read-only preview results retain `safe_to_attach: false` and `attachment_attempted: false`.

## 2026-11-02 — Explicit x86 / ARM64 mismatch classification for the x64 DLL

Intimatr currently builds/ships an x86-64 Windows DLL for Wine. The **read-only Bottles path-matching report** now includes `x64_dll_compatible` as a *machine-bit-only* result: `true` only when **both** a verified running process machine and a verified on-disk PE file machine are x86-64, `false` when both machine observations exist but are not both x86-64, and `null` when evidence is incomplete. This flag **does not establish** that hooks, runner, DLL version, memory permissions, target identity or game semantics are supported.

When both verified machines are x86 (or both ARM64), a matched registered executable is labeled `unsupported_x64_dll_architecture`, and the command suppresses registered program selection. When file and process machine conflict, `process_file_architecture_conflict` takes precedence. In every case `safe_to_attach` remains false. No 32-bit DLL or ARM64 DLL has been built, and this output must not advertise support for those targets.

Synthetic CI adds a registered second program with a known **x86** Windows PID/image snapshot, and requires `x64_dll_compatible: false`, an explicit unsupported status, a null selected program and no attachment. Actual native Bottles x86 game testing remains open.

## 2026-11-02 — Explicit Windows PID and full-executable preflight (review only)

The native `intimatr-bottles live-match` command also accepts an optional **paired** `--pid WINDOWS_PID --exe ABSOLUTE_WINDOWS_PATH` selection. Supplying just one selector, a zero PID, or an invalid drive-qualified executable path fails closed. The selected PID must be present in the current Wine read-only snapshot, and the supplied executable must match the *full observed image path*, not just its basename. The same path must uniquely match a registered program in the selected Bottles inventory.

```bash
intimatr-bottles live-match \
  --bottle 'Offline Games' \
  --helper /absolute/path/wine-process-list.exe \
  --runner /absolute/path/to/wine \
  --pid 42 \
  --exe 'C:\\Games\\Example\\Example.exe'
```

When both selectors are provided, the result includes `selected_candidate` with `registered_identity_verified_at_snapshot`, `x64_dll_compatible`, the exact observed/selected paths and a reasoned `status`. Possible results distinguish a missing/recycled PID, wrong path, unregistered/ambiguous program, unknown architecture, incompatible x86/ARM64, or an x64 candidate for **further review only**. This is a *snapshot* with an unavoidable time-of-check/time-of-use window; an earlier matching PID is never a durable authorization token.

Every result keeps `snapshot_only: true`, `safe_to_attach: false`, and `attachment_attempted: false`. The new selector does not run an attach helper, activate arbitrary-game hooks, modify games, or confer permission to inject. The existing `live-match` behavior without selectors is unchanged. Deterministic Rust fixtures cover exact matches, missing/wrong PIDs and paths, duplicate registrations, missing bitness, and unsupported architectures. A Wine CI fixture is also being extended to check the full selected PID/path command against an already-running disposable test process; actual native Bottles installation and offline-game field QA remain open.

## 2026-11-02 — Fixture identity hardened against same-named executables

The standalone Windows attach helper no longer accepts a target solely because the image basename is `wine-attach-target.exe`. Before any remote-memory allocation or loader-thread creation, it queries the selected process handle's **complete image path**, canonicalizes that path and the deliberately owned test executable beside `wine-attach-helper.exe`, and requires exact path equality (ASCII-case-insensitive under Windows). Missing or unresolvable image paths fail closed. The helper still retains its process handle, per-target lock, duplicate-load refusal and post-load module confirmation; it does not gain any general-game permission.

A Windows-only synthetic regression checks that an unrelated executable with the **same filename** in another directory is rejected. The [Wine CI smoke run 41558365727](https://github.com/sguzman/intimatr/actions/runs/41558365727) additionally passed the owned-target load test and a real separate same-named Wine decoy rejection. This was **Wine integration evidence**, not a full-green run: that revision still had a strict Clippy test-placement failure, which was corrected in subsequent commits. This check is a fixture boundary, not a content hash, mapped-process attestation, security proof against races, or production game-attachment authorization. INT-1407 remains open and verified expansion remains 16%.

## 2026-11-02 — Fail-closed module inventory and explicit fixture runner

The controlled `wine-attach-helper.exe` scans the selected Windows PID's module list before remotely loading `intimatr.dll`. Previously a failed `Module32NextW` call was treated as an ordinary end-of-list; this could incorrectly report the DLL as absent after a truncated scan. The helper now checks `GetLastError` **immediately** after enumeration stops and treats only `ERROR_NO_MORE_FILES` as a complete list. Any other result aborts with an explicit enumeration error, before an attach decision. Deterministic Windows tests cover expected end-of-list and error results.

Likewise, the Linux `intimatr-bottles probe-attach` fixture-only command now **requires** `--runner EXPLICIT_WINE_BINARY`. It no longer silently invokes the host's default `wine` for a selected prefix. The existing Wine CI smoke calls all supply a runner, and a new negative check rejects omission before starting a Windows helper. This makes runner selection consistent with `live-match` and `win-processes`; it still does not prove that the chosen runner equals a real bottle's configured runner.

Both changes remain within disposable fixture testing. The module enumeration result is not an anti-race guarantee; real-game loader eligibility, production hook compatibility, native Bottles field QA and independent status attestation remain open. All three CI jobs passed in [run 41560000448](https://github.com/sguzman/intimatr/actions/runs/41560000448). Expansion stays 16% (+0 pp).

## 2026-11-02 — Exact Intimatr DLL module identity after remote fixture load

Module presence and **module identity** are different checks. The fixture loader already refused duplicate loads when the target module inventory contained `intimatr.dll`. However, its post-load check previously accepted a matching basename alone. This could falsely claim successful attachment if another file named `intimatr.dll` appeared between the initial check and the remote loader thread.

The Windows-only fixture helper now classifies a same-named observed module using the complete path supplied by `MODULEENTRY32W`, comparing its canonical path against the **specific `intimatr.dll` beside the helper**. Before remote loading, any pre-existing same-named module is refused, regardless of its path; after loading, only the expected owned file counts as successful confirmation. Invalid UTF-16, missing paths, failed canonicalization and incomplete enumeration fail closed. Synthetic Windows tests cover matching, same-named different-path, and unresolvable DLL paths. The existing Wine smoke continues to exercise the positive owned-DLL load and independently observed target lifecycle. Latest CI for this exact-path extension remains pending.

This is an additional safeguard for disposable owned fixtures, **not** a signature check, a remote process attestation, proof of runner compatibility or authorization to attach to a game. Real offline-game support and INT-1407 remain unfinished; no new weighted points are earned.

## 2026-11-02 — Shared live-machine identification and x64 remote-load gate

`wine-process-list.exe` and `wine-attach-helper.exe` now use a shared Windows `windows_process_machine` module. It obtains the **actual running process's** machine using dynamically discovered `IsWow64Process2`, with no guessed fallback if the API is missing, fails, or returns unknown machine codes. Unit tests cover native x64, x86 WoW64, ARM64 and unsupported values. The read-only PID inventory retains its existing nullable architecture metadata.

Before creating a remote loader thread or modifying remote memory, the **disposable fixture-only attach helper** now checks the held target process handle and refuses it unless the live machine is positively identified as `x86_64`. This is independent of the target's basename, full image path and on-disk PE header. The tested Wine runner supports this Win32 query, but other Wine runners may not; unknown evidence is a refusal, **not** authorization to try an unsafe cross-bitness injection. The live Windows/PE architecture report and the fixture-only attach helper now share decoding semantics.

The positive Wine fixture and cross-Windows CI for this new gate were running when this section was written. Real native Bottles runner selection, actual game process ownership, x86 builds, offline-game safety and general timed-hook compatibility remain unverified. No general game attachment was enabled, and the weighted expansion stays at 16/100.

## Combined native Linux + Wine bundle (experimental)

The Linux cross-build CI now produces `intimatr-linux-wine-x86_64-VERSION.tar.gz` alongside its archive SHA-256 sidecar. The verified archive contains the native `bin/intimatr-bottles` ELF, `wine/intimatr.dll`, `wine/intimatr-speed.exe`, read-only `wine/wine-process-list.exe`, examples, version marker, per-file SHA-256 checksums, and reference docs. To build it after compiling the native CLI plus Windows GNU binaries:

```bash
bash scripts/package-linux-release.sh test-1
bash scripts/verify-linux-release.sh dist/intimatr-linux-wine-x86_64-test-1.tar.gz test-1
```

The verifier checks file presence, archive and member checksums, version, the Linux ELF and Windows x86-64 PE signatures, and rejects symlink entries. The archive does **not** enable the experimental hook in arbitrary games, automatically enroll Bottles, claim a production-safe attach/unload, or verify the owner's installed Wine runner. Native Bottles field QA and owner acceptance remain required. This is a standalone CI/release *packaging* milestone only and does not earn the broader INT-1702 acceptance weight by itself.


## 2026-11-02 — Single-result attachment readiness report for a selected PID

The read-only `intimatr-bottles live-match` command now emits an `attachment_readiness` object when **both** `--pid` and `--exe` select one running Windows process. This combines the separate Bottles prefix ownership, native runner configuration, exact registered PID/path, and running-process / PE image architecture checks into a deterministic, machine-readable list of **blockers**.

The command is the same as the existing explicit candidate preflight:

```fish
intimatr-bottles live-match --bottle Games \
  --helper /absolute/path/to/wine-process-list.exe \
  --runner /absolute/path/to/configured/wine \
  --pid 42 --exe 'C:\\Games\\Example.exe'
```

In addition to `selected_candidate`, the JSON now has `attachment_readiness` with `candidate_status`, `snapshot_registered_identity_verified`, `snapshot_x64_dll_compatible`, `bottle_prefix_verified`, `configured_runner_verified`, `game_process_prefix_ownership_verified`, `status`, `blockers`, `safe_to_attach`, and `attachment_attempted`. The recorded process ID above is illustrative; it must be taken from a fresh process list, never hard-coded.

Even when x64/PID/path match, the report intentionally blocks attachment until the selected runner is independently proven to be the bottle's configured runner, the **live** target process is tied to that prefix, a fresh apply-time identity check exists, protected/anti-cheat programs are excluded, and the production loader/lifecycle is implemented and tested. Bottles storage ownership alone is **not** live process ownership. No `apply` command or general-game helper is exposed by this change, and no DLL is injected by `live-match`.

A portable four-case Rust test suite covers positive snapshot candidates, nonexistent PID / unverified architecture, a synthetic wrong bottle/x86 target, and the fact that even all supplied positive identity flags cannot grant attachment. The Wine CI suite checks the JSON against a genuinely running disposable fixture and rejects missing PID and wrong full-path candidates. This is INT-1403/1407 staged work; neither milestone's full acceptance is complete. **Expansion remains 16% (previous 16%, Δ +0 pp).**


## 2026-11-02 — Two independent Wine prefix namespace regression

The existing `intimatr-bottles live-match --bottle ... --pid ... --exe ...` read-only candidate now separates **Windows PID observed** from **that PID's exact full executable path observed** in the snapshot obtained through a Windows helper launched with the selected, verified `WINEPREFIX`. A nonexistent PID has both booleans false; a present PID with a wrong selected path reports PID present but image mismatch. The downstream `attachment_readiness` report exposes those same distinct facts and their blockers instead of collapsing them into a generic registered-game match.

The isolated Wine test now sets up two fresh prefixes using the same runner, launches two separate disposable Windows PE fixture executables with the same **basename** from different directories, obtains Windows process inventories independently under each prefix, and requires that each snapshot include its own PID and full image but **exclude the other** prefix's image. It does not rely on Windows PID integers being globally unique across independent wineservers. This is a cross-prefix isolation regression on CI's tested Wine version, not a promise about all patched Wine builds or a Linux-PID-to-Windows-PID mapping.

This improves the evidence that the explicit snapshot is scoped to the requested Wine session but does **not** prove that a game's Linux process belongs to the bottle, that a manually supplied runner equals Bottles' configured runner, or that a later PID still refers to the same game. The program continues to report `game_process_identity_verified=false` and `safe_to_attach=false`. No game was remotely loaded or mutated. CI for this new regression is pending. Expansion remains **16/100 (previous 16, Δ +0 pp)**.


## 2026-11-02 — Native Linux controller responsibilities separated

The native `intimatr-bottles` binary retains its existing subcommands but no longer owns four unrelated workflows in its entrypoint. The binary's central file focuses on command routing, read-only Bottles scan/planning and shared Windows process-snapshot launching; the following independently testable command modules live under `src/bin/intimatr_bottles/`:

- `probe_attach.rs`: strictly disposable fixture remote loader CLI, explicit runner and prefix, no general-game loading.
- `profile_check.rs`: read-only global/bottle/executable speed profile resolution.
- `read_only_snapshot.rs`: offline Bottles programs and Windows process snapshot reconciliation.
- `live_match.rs`: scoped native Bottles inventory, runner and prefix audit, live Windows process matching, and attachment-readiness reporting.

These modules use ordinary Rust module boundaries, not source-code includes or a generated monolithic file. The extraction preserves command flags and JSON contracts, including `safe_to_attach=false` for all live-game reviews. Module-only Rust tests moved with their code. The structure change does **not** constitute a verified game attach. Strict Linux/Windows formatting, Clippy, unit and Wine integration CI results were pending at this checkpoint; expansion stays **16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Pin Wine by absolute executable path

The Linux `win-processes`, `live-match`, and fixture-only `probe-attach` commands now require `--runner` to point to an **absolute, existing, executable file**. A bare `--runner wine` or `--runner wine64` is rejected even if it resolves on `PATH`, avoiding accidental substitution of the host's Wine for a patched Bottles runner. Existing symlink aliases canonicalize to their target file; missing, directory and non-executable paths fail before Wine is launched. Absolute paths containing spaces are supported.

`live-match` separately emits `runner_binary` and `runner_binary_verified=true` to report the resolved file. This is **not** the same as `runner_verified` on the Bottles audit: an executable's existence does not establish that the game/bottle actually used it. The CLI has no general game loader, and this guard does not authorize attachment.

For disposable CI tests, the absolute system Wine is obtained from `command -v wine`. For an actual Bottles game, supply the executable belonging to its selected patched runner rather than relying on `PATH`. CI for this hardening is pending; expansion stays **16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Native managed Bottles runner identity check

For ordinary **native** Bottles managed runners, the read-only `live-match` command now checks the selected `Runner` from native Bottles inventory against the managed runner directory that upstream Bottles itself derives from `XDG_DATA_HOME/bottles/runners/<Runner>`. If `XDG_DATA_HOME` is unset, the native default is `$HOME/.local/share`. The path `<Runner>/bin/wine` must resolve to the **same executable file** explicitly passed as `--runner`. Directory and executable ownership are checked without running Bottles' `list components` command, which can perform runner maintenance.

This applies only when Bottles' configured runner name is a safe single directory component, the managed directory exists as an ordinary direct child of that root, the requested executable is the same canonical file, and Steam/host-system/recorded-external runner exceptions do not apply. If the managed location is missing, the result stays **unverified** instead of guessing. A different file is an error before the probe runs. The existing separately verified Steam `RunnerPath` case remains unchanged.

The resulting `runner_verified=true` is **configuration-path evidence** for the active native Bottles CLI environment, not proof that an already-running game used this runner or that it's safe to modify. Game process identity, protection state, loader and hook cleanup remain unsolved, and `safe_to_attach` remains false. Unit tests use synthetic managed runners, and Wine CI uses a disposable managed root with a symlink to its real system Wine; a wrong runner is refused. Native user's Bottles compatibility still needs actual installation QA. **Expansion remains 16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Experimental explicit offline-game attachment

The `intimatr-bottles attach-game` CLI and separately built `wine-game-attach-helper.exe` now offer a single-target, opt-in path for an offline single-player executable registered in native Bottles. The existing `probe-attach` remains fixture-only. The manager requires a verified native Bottles prefix, the configured executable for a managed or recorded Wine runner, a freshly observed Windows PID plus exact registered full image path, matching observed x86-64 PE/live architecture, and `--offline-single-player`. The Windows helper rechecks the selected image on a held process handle under a per-PID lock, refuses duplicate Intimatr loads and obvious protected modules, and confirms that the exact Intimatr DLL appears in the target after loading. A protection-module scan is only a conservative veto, not proof of an unprotected game.

The mixed Linux/Wine artifact now includes `bin/intimatr-bottles`, `wine/wine-process-list.exe`, `wine/wine-game-attach-helper.exe` and `wine/intimatr.dll`. Use the **same native Bottles runner** for inspecting and attaching. The following is a template, not a command to run without a known offline game:

```fish
./bin/intimatr-bottles live-match --bottle 'Bottle Name' \
  --helper (pwd)/wine/wine-process-list.exe \
  --runner /absolute/path/to/runner/bin/wine

./bin/intimatr-bottles attach-game --bottle 'Bottle Name' \
  --helper (pwd)/wine/wine-process-list.exe \
  --attach-helper (pwd)/wine/wine-game-attach-helper.exe \
  --runner /absolute/path/to/runner/bin/wine \
  --pid WINDOWS_PID --exe 'C:\Games\Offline.exe' \
  --offline-single-player
```

Choose the Windows PID and full image path from the live-match report, **not** a Linux PID or guessed basename. The positive `intimatr_module_loaded` result is deliberately not a claim that game timer interception, gameplay speed control, or production teardown work; `speed_controls_verified` remains false. The helper loads no DLL into personal games in CI: integration uses separate disposable PE test targets. Actual native Bottles QA, measured speed changes in a real game, protection compatibility and safe lifecycle recovery remain outstanding. **Expansion: 16/100 (previous 16, Δ +0 pp).**

## 2026-11-03 — Windows process-generation identity for opted-in game attachment

The read-only `wine-process-list.exe` now captures the Windows `GetProcessTimes` creation FILETIME for each process on the **same HANDLE** used for its running-image and architecture inspection. It serializes `process_creation_time_100ns` as a decimal **string**, not a JSON number, to preserve all 64 bits without JavaScript/jq floating-point rounding. The value propagates through registered full-image matching and the explicit `live-match` candidate report.

`attach-game` refuses a game when this creation identity is missing. It forwards the exact snapshot creation time to the dedicated Windows game helper as `--created-100ns`, which opens the chosen PID and rechecks creation time on the held process HANDLE both before and after acquiring the existing attach mutex. Thus a recycled PID with the same executable path cannot silently pass. The fixture-only loader remains unchanged and does not require this game-only argument. A Linux/Wine regression supplies a deliberately incorrect generation for the still-running disposable target and requires refusal before the positive attachment.

This addresses a distinct PID reuse hazard but **does not** demonstrate gameplay speed control, completeness of protection checks, or safe production hook shutdown. The read-only readiness report continues to set `safe_to_attach=false`. Native Bottles real-game QA remains outstanding; expansion **16/100 (previous 16, Δ +0 pp)** until full roadmap acceptance.

### Explicit selected-process generation (2026-11-03)

For a mutable `attach-game` request, the user must supply **three** selectors taken from `live-match`: `--pid`, `--exe`, and `--created-100ns` (the decimal string in `selected_candidate.process_creation_time_100ns`). The controller obtains a new Wine snapshot and refuses if the selected creation timestamp differs, even when the Windows PID and executable path match. The helper independently compares the same creation stamp on its held Windows handle before writing memory. This closes both the operator-selection-to-snapshot and snapshot-to-load PID-reuse windows as far as the exposed Windows creation identity supports. Never guess the stamp or turn it into a floating-point JSON number.

Example additional flag for the previously documented `attach-game` command:

```fish
  --pid WINDOWS_PID --exe 'C:\Games\Offline.exe' \
  --created-100ns 'FILETIME_FROM_LIVE_MATCH' --offline-single-player
```

The previous two-selector example is now intentionally rejected by `attach-game`. This does not alter the read-only `live-match` command or the fixture-only loader. Real-game speed and shutdown proof remain unfinished. **Verified expansion 16/100 (previous 16, Δ +0 pp).**

## 2026-11-03 — Experimental four-clock PE static import preflight

The native Bottles live-process inventory now reports `timer_import_coverage` based on a **bounded, read-only inspection of the selected running executable's on-disk x64 PE import tables**. Separate booleans identify statically imported `QueryPerformanceCounter`, `GetTickCount64`, `GetTickCount`, and `timeGetTime`, and `all_four_static_names` is true only when all four imported symbol names were observed in recognized Windows system DLL import descriptors. PE section/RVA bounds, descriptor/thunk limits, invalid headers and missing terminators fail closed to `null` (unknown) rather than fabricating an unsupported verdict. A readable image without a static import table has known-empty coverage. This is **not** an active hook capability test: Wine may resolve imports differently, the application may use dynamically resolved APIs, and timing may be performed in DLLs or the rendering engine rather than the main executable.

The evidence propagates through `win-processes`, `live-match` candidate selection, and `attachment_readiness`; missing or incomplete coverage adds the informative `experimental_four_timer_static_imports_unverified` blocker, but does not itself authorize or install hooks. Deterministic PE table tests cover all-four, missing, invalid and empty imports. The Wine fixture regression requires all four static import names from the running disposable test target. Production timer interception remains fixture-only, so real-game speed is **not** yet enabled. Expansion remains **16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Independently verify target DLL runtime Running

The explicit `attach-game` path no longer equates a successful remote `LoadLibraryW` and expected DLL module-list presence with a functioning Intimatr runtime. The DLL exports a **side-effect-free `intimatr_remote_lifecycle_query(void*) -> u32`** using the Windows thread-start ABI. The Windows game helper maps the *exact owned DLL* without executing its initializer, bounds-checks that export's address within the PE image, resolves the same RVA inside the target's verified exact module, and invokes the query on a bounded target-side thread. It requires the target lifecycle to reach `Running` (state 3) and rejects `Failed`, `Stopping`, `Stopped`, an unknown state or any unreadable/timeout result. The ordinary fixture helper remains unchanged.

Only after this target-side evidence does `attach-game` emit `runtime_running_verified=true`, and only if it receives the dedicated success marker from the helper. If a module loads but bootstrap fails or cannot be queried, **loading may already have happened**; the command fails with a diagnostic rather than claiming success or retrying injection. The DLL is not automatically unloaded, because a safe runtime rollback path for arbitrary game processes is still unverified. This verifies **initialization**, not timer interception: `speed_controls_verified` stays false and the IAT hook backend remains fixture-only. Wine CI checks the reported Running state in the separately located disposable target. Expansion **16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Stop an explicitly attached game runtime without unloading DLL

`intimatr-bottles stop-game` accepts exactly the same explicit `--bottle`, `--pid`, `--exe`, `--created-100ns`, `--runner`, `--helper`, `--attach-helper`, and `--offline-single-player` selection as `attach-game`. It repeats native Bottles runner/prefix/program association checks, and the Windows game helper revalidates the held process image, creation time, architecture and exact owned Intimatr DLL under the per-target mutex. The helper never calls `LoadLibraryW` in stop mode. It calls the new ABI-correct `intimatr_remote_shutdown(void*)` export and independently requires `intimatr_remote_lifecycle_query` to return `Stopped`.

A success report explicitly includes `runtime_stopped_verified=true`, `shutdown_attempted=true`, `intimatr_module_loaded=true`, and `intimatr_module_unloaded=false`. No `FreeLibrary` or forced game exit is attempted, because a thread can still retain an old callback address after import restoration. Stop is repeatable, but a stopped module remains loaded and a fresh attach cannot happen until game exit/relaunch. This is **not proof of universal production-game hook rollback**; the generic game hook remains disabled. Wine CI exercises stop and repeat-stop against a disposable loaded PE target after its four-clock fixture hook restoration. Expansion **16/100 (previous 16, Δ +0 pp)**.

## 2026-11-03 — Explicit target-side status without duplicate attachment

`intimatr-bottles status-game` accepts the same exact, opted-in target selection as `attach-game` and `stop-game`, including the live-match process creation timestamp. The Linux manager refreshes the selected native Bottles prefix and managed-runner evidence; the Windows helper rechecks the held target process image, generation, and architecture under the per-PID mutex. If the selected target has no Intimatr module, the report has `runtime_state="absent"` and `intimatr_module_loaded=false`. An identically named DLL from another path is rejected, not queried.

When the exact owned DLL is loaded, the helper executes only its bounded, side-effect-free lifecycle query on a short-lived remote thread and reports the observed state: `detached`, `attached`, `bootstrapping`, `running`, `stopping`, `stopped`, or `failed`. Unknown states, an inaccessible process, an incomplete module scan, or a timed-out query fail closed. Unlike `attach-game` and `stop-game`, this operation does not load or unload the DLL, change game speed, or request shutdown. Creating the inspection thread is itself a target-process operation, so it is not a zero-interaction external scan.

The report keeps `speed_controls_verified=false`: the DLL lifecycle alone cannot establish installed timer hooks or accelerated gameplay. Regression coverage in the Wine job tests absent, running, and stopped states against the disposable PE attach target; it does not claim production-game compatibility or advance an unfinished weighted milestone by itself.

## 2026-11-03 — Distinguish installed four-timer hooks from DLL lifecycle

The explicit `status-game` response now separately reports `timer_hook_state`: `none`, `installed_four_timers`, or `cleanup_pending`. Its `timer_hooks_installed_verified` field becomes true only if an independently queried live target reports both `Running` and a complete four-API IAT hook set with its callback DLL pinned. Partial restoration, a failed install retaining page protections, and a shutdown request never count as installed; poisoned or unknown hook states fail closed. An unattached process reports `none` without invoking a DLL export. The exact selected PID, process creation timestamp, image, managed Wine runner, and Bottles prefix checks remain mandatory.

These checks **do not** claim measured gameplay acceleration, generic-game compatibility, or operational speed controls: `speed_controls_verified` remains false, and ordinary games remain ineligible for timer-hook activation. The disposable Wine target's four imports are inspected externally at the installed stage, after their restoration, and after target runtime shutdown, while existing timing tests still measure 0.5×, 2×, and 1× independently. This is a prerequisite for trustworthy future applied-speed status and production activation, not completion of either milestone.

The exact-target Wine fixture additionally supports a test-only remote-stop mode: after proving 0.5×/2×/1× clock changes, it deliberately retains installed hooks at 2× rather than disabling them itself. `stop-game` must then restore all imports and independently report `Stopped`, while `status-game` transitions from `installed_four_timers` to `none`. The DLL stays mapped. This validates the external rollback path in a disposable PE test process; it does not substitute for real-game or multicore stress QA.
