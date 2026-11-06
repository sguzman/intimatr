# Intimatr Milestones

**Current expansion baseline (2026-11-06): 19/110 weighted points (17% rounded).** This includes 16 points verified on the original 100-point speed roadmap plus 3 points for the separately verified MangoHud telemetry channel. The newly requested HUD extends scope by 10 points; earlier 16/100 entries below are historical checkpoints, not current full-scope percentages. The latest batch HUD and release-packaging changes are pending CI and receive no additional credit yet.

Work is ordered by dependency: later phases should build on the shared core instead of bypassing it.

## 0. Foundation

- [x] Convert the crate into a Rust `cdylib`/`rlib` core suitable for producing `intimatr.dll`.
- [x] Define the one-TOML-per-executable configuration convention.
- [x] Add validated configuration models for target, logging, RPC, scanner, debugger, UI, and policy settings.
- [x] Add non-blocking structured file logging with optional console output.
- [x] Define CE-style scan predicates: exact, comparison, range, changed/unchanged, increased/decreased, and delta predicates.
- [x] Add tests for configuration resolution/validation and predicate semantics.
- [x] Add CI for formatting, Clippy, and tests.
- [x] Add initial project specification and build documentation.

## 1. Windows DLL bootstrap and lifecycle

- [x] Add the minimal Windows DLL entrypoint and keep loader-lock work trivial.
- [x] Move initialization onto a dedicated bootstrap thread after process attach.
- [x] Resolve the host executable path and automatically load its per-game TOML configuration.
- [x] Initialize tracing before subsystem startup and log lifecycle transitions extensively.
- [x] Add panic containment around FFI/thread entry boundaries.
- [x] Add explicit lifecycle states and a clean shutdown path.
- [x] Add lifecycle/path-resolution tests and keep Windows FFI isolated behind auditable modules.

## 2. Memory engine and CE-style scanner

- [x] Enumerate committed memory regions with `VirtualQuery` and normalize region metadata.
- [x] Apply readable/writable/executable/guard-page filters from configuration.
- [x] Add safe wrappers for typed in-process reads.
- [x] Add policy-gated typed writes with temporary protection changes where needed.
- [x] Implement first-scan traversal with chunking and alignment controls.
- [x] Store typed scan candidates and previous-value snapshots.
- [x] Implement next-scan filtering using all predicates already defined in `scanner::predicate`.
- [x] Support signed integers, unsigned integers, `f32`, and `f64` with explicit data-width metadata.
- [x] Add unknown-initial-value scans for CE-style discovery workflows.
- [x] Add scan cancellation, result limits, progress reporting, and timing/throughput logs.
- [x] Add deterministic scanner tests over synthetic memory buffers plus Windows backend smoke tests.

## 3. Shared command layer and RPC

- [x] Define frontend-neutral commands/results for modules, memory, scans, watches, debugger operations, and lifecycle.
- [x] Enforce policy gates in the shared command layer rather than individually in frontends.
- [x] Define a versioned serialized RPC protocol.
- [x] Implement framed localhost TCP transport.
- [x] Implement optional Windows named-pipe transport.
- [x] Add connection/request size limits and structured per-request tracing spans.
- [x] Add RPC integration tests using an in-memory or loopback client/server pair.
- [x] Add a small first-party Rust client crate/example to prove arbitrary external frontend support.

## 4. In-process CE-style UI

- [x] Select and integrate the Rust GUI/rendering stack without doing heavy work in `DllMain`.
- [x] Add native tool-window visibility/hotkey handling from TOML.
- [x] Add memory scan controls and result table.
- [x] Add watch/freeze list backed by the shared command layer.
- [x] Add memory viewer/editor.
- [x] Add module/thread browser.
- [x] Add persistent UI layout/state per target executable where useful.

## 5. Debugger core and UI

- [x] Enumerate threads and expose register/context snapshots.
- [x] Integrate an x86/x64 disassembler.
- [x] Add instruction/memory views and navigation.
- [x] Implement hardware breakpoint management.
- [x] Implement exception-based breakpoint handling with carefully scoped VEH logic.
- [x] Add continue/pause/single-step state machinery where viable in-process.
- [x] Surface debugger events through both the UI and RPC stream.
- [x] Add debugger-focused tests around state transitions and breakpoint bookkeeping.

## 6. Advanced search and analysis

- [x] Add array-of-bytes/pattern scanning with wildcards.
- [x] Add pointer-chain search and validation.
- [x] Add module-relative/symbolic address expressions.
- [x] Add saved scan sessions and reusable watch definitions.
- [x] Add structure-oriented memory inspection helpers.
- [x] Add scriptable automation through the RPC command API without duplicating core logic.

## 7. Hardening and releases

- [x] Benchmark large scans and optimize allocation/chunking hot paths.
- [x] Add bounded queues/backpressure between scanner, UI, debugger, and RPC tasks.
- [x] Add crash-safe shutdown and log flushing.
- [x] Add Windows x86_64 CI builds that verify the DLL artifact.
- [x] Add release packaging with example configs and protocol/version notes.
- [x] Document frontend/plugin extension points and subsystem invariants.

## 8. Release candidate usability

- [x] Add an archive verifier that checks release layout, version metadata, and the packaged DLL checksum.
- [x] Make CI validate the exact release ZIP produced by the packaging script.
- [x] Publish an archive-level SHA-256 sidecar alongside tagged release ZIPs.
- [x] Add a first-run quick start from package layout through configuration, UI scanning, RPC, persistence, and shutdown.
- [x] Add troubleshooting guidance for bootstrap, configuration, UI/debugger policy gates, scanning, RPC, stale addresses, and lifecycle behavior.

## 9. Field-test scan hardening

- [x] Parallelize scalar first scans across memory chunks with a dedicated configurable worker group.
- [x] Parallelize historical next scans across candidate batches while preserving sorted result ordering.
- [x] Expose live active-scan progress through the shared command layer.
- [x] Show live progress/throughput and a persistent running-scan indicator in the native UI.
- [x] Allow cancellation of both first and next scans from the native UI.
- [x] Keep unrelated UI errors from obscuring whether a scan is still active.
- [x] Isolate every process run into its own diagnostic log file so field-test logs never append across launches.

## 10. Speed-control expansion — feasibility and design 🔍
**Status:** active (6/9 points verified). **Weight:** 9/100 expansion points.
- [x] INT-1001 🔎 Inventory regression baseline and integration seams.
- [x] INT-1002 ⏱️ Prove timing-hook feasibility in isolated Wine QPC IAT test (0.500×/2.000×/1.000×, CI 41367449695; restricted fixture only).
- [ ] INT-1003 🍷 Prove Bottles/Wine loader and configuration-path strategy (active; isolated 64-bit Wine LoadLibrary smoke passed, live attach/path proof outstanding).

## 11. Time virtualization engine ⏩
**Status:** active (4/19 points verified). **Weight:** 19/100 expansion points.
- [x] INT-1101 🧮 Pure virtual clock model (Linux CI 41357696793, cross-target build and synthetic tests).
- [ ] INT-1102 🪝 Windows timing-hook backend.
- [ ] INT-1103 🧭 Clock consistency and live multiplier transitions (active; selected-fixture 0.5×/2×/1× IAT timing transitions driven by the native Linux CLI and shared command executor under Wine; real game QA pending).
- [ ] INT-1104 🧯 Safe hook lifecycle and fallback.
- [ ] INT-1105 🎮 Offline Wine game proof.

## 12. Shared configuration and profiles ⚙️
**Status:** active (3/12 points verified). **Weight:** 12/100 expansion points.
- [x] INT-1201 🗂️ Global versioned speed profile (validated TOML, Linux CI 41357980641).
- [ ] INT-1202 🧩 Per-bottle/per-game matching (active; deterministic TOML override parser, case/path identity tests and read-only Linux preview implemented; native Bottles validation missing).
- [ ] INT-1203 🔗 Wine-visible shared configuration path (active; explicit DLL-side environment path and bounded global preview loader implemented; verified Bottles identity and cross-game handoff pending).
- [ ] INT-1204 🔄 Reload, validation, persistence.

## 13. Hotkeys and speed modulation ⌨️
**Status:** active (3/10 points verified). **Weight:** 10/100 expansion points.
- [x] INT-1301 🎚️ Step/preset/toggle/reset state machine (Linux CI 41358317446 core unit tests).
- [ ] INT-1302 ✋ Hold-to-modulate and optional startup speed (active; bounded holds/startup expiry and opt-in preview focus worker wired; live in-game acceleration, modifier behavior and recovery still unverified).
- [ ] INT-1303 🔑 Configurable, conflict-aware hotkeys (active; opt-in Win32 focused-window preview worker, command state handoff and unambiguous HWND selection implemented; real Wine game input/Wayland field QA pending).
- [ ] INT-1304 🧷 Authoritative status and recovery (active; live installed-IAT-hook status and virtual clock multiplier separately queryable, but measured gameplay speed and real-game recovery not yet verified).

## 14. Bottles deployment manager 🍾
**Status:** active (0/23 points verified; native read-only inventory implemented, real Bottles QA pending). **Weight:** 23/100 expansion points.
- [ ] INT-1401 🔭 Discover native Bottles and games (active; Linux /proc prefix records and same-prefix Windows PID enumeration verified in fixtures, real native Bottles installation not yet inspected).
- [ ] INT-1402 🧲 Include/exclude filters (active; read-only selection CLI and synthetic fixture tests implemented, native Bottles field QA pending).
- [ ] INT-1403 📋 Non-mutating plan; apply/status operations (active; explicit attach/arm/preset/status/stop exercised against Wine fixture, actual Bottles field QA pending).
- [ ] INT-1404 🔌 Verified normal-launch DLL loading.
- [ ] INT-1405 🛟 Idempotence, backups and rollback (including process attach).
- [ ] INT-1406 🧱 32-/64-bit, Linux GNU build and runner compatibility reporting.
- [ ] INT-1407 🧷 Explicit attach to already-running Wine processes via native Linux manager (active; same-PID competing helpers serialized in disposable Wine CI; explicit PID/full-image read-only preflight added with fail-closed checks; real Bottles game, attachment authorization, and cleanup remain unverified).

## 15. Unified UI/RPC 🖥️
**Status:** active (0/7 points verified; preview-only speed command DTOs and shared executor integrated, RPC integration QA pending). **Weight:** 7/100 expansion points.
- [ ] INT-1501 🧠 Shared command and RPC interface (active; CLI preset commands now synchronize a selected fixture's IAT clock through the same command executor as RPC/UI; production-game behavior and full frontend QA pending).
- [ ] INT-1502 🪟 Native egui controls and appearance/keyboard QA.
- [ ] INT-1503 📟 Status without keeping the scanner foregrounded (active; exact-target lifecycle, IAT installation and installed virtual-clock multiplier available; gameplay-rate verification and real native Bottles QA outstanding).

## 16. Verification and hardening 🧪
**Status:** pending. **Weight:** 15/100 expansion points.
- [ ] INT-1601 🧫 Deterministic core/config/input tests.
- [ ] INT-1602 🍷 Real Bottles/Wine compatibility matrix.
- [ ] INT-1603 🩺 Scanner/debugger/RPC regression evidence.
- [ ] INT-1604 🔐 Failure recovery and rollback QA.

## 17. Documentation and acceptance 📦
**Status:** pending. **Weight:** 5/100 expansion points.
- [ ] INT-1701 📚 Configuration/usage/troubleshooting reference.
- [ ] INT-1702 📦 Linux-built Windows PE DLL, native Linux CLI, and verified CI/package contents.
- [ ] INT-1703 🏁 End-to-end owner acceptance checkpoint.

## 18. Optional in-game performance HUD 🎛️
**Status:** active (3/10 added points verified). **Weight:** 10 points added to the original 100-point expansion, yielding a 110-point full-scope denominator.
- [x] INT-1801 🖧 Read-only installed-hook-rate export to MangoHud (3 pts; Windows/Wine fixture tests passed, CI 42230898473).
- [ ] INT-1802 🎛️ Reversible per-game MangoHud configs and live/missing rate regression (3 pts; implemented, latest CI pending).
- [ ] INT-1803 🍾 Actual Bottles/Wayland/MangoHud overlay and speed-change QA (3 pts; native-game evidence outstanding).
- [ ] INT-1804 📦 Self-contained Linux/Wine release including batch proxy and HUD tools (1 pt; packaging updated, verification pending).

**Expanded roadmap and precise task acceptance criteria:** [Speed Control and Bottles Integration Roadmap](docs/speed-control-roadmap.md).

**Progress basis:** Milestones 0–9 retain their historical checked state and are not included in the new 100-point expansion denominator. All milestone 10–17 deliverables started pending. The repository baseline inspection INT-1001 is verified against source and historical CI, earning 3/100 weighted points; Linux-hosted PE build and explicit already-running Wine-process attachment are now firm requirements, with M14 reweighted to accommodate INT-1407 while retaining the 100-point overall denominator; expansion progress is **13%** (previous session 3%, delta +10 percentage points) after verifying INT-1101 pure clock, INT-1201 global speed profile and INT-1301 modulation state machine. INT-1401 has source and CI in progress but earns no weight until real native Bottles inventory is validated. The pure virtual clock and Linux-hosted x86-64 GNU DLL cross-build have been verified through CI; no Windows timer hook or Wine process attachment is verified. Planning/documentation alone does not complete the features. Confirmed changes must satisfy each leaf item's tests and evidence before gaining weighted credit.

**2026-11-02 continued development checkpoint:** timer API projection and shared speed session tests, safe Bottles selection plans, and an isolated `wine-load-probe` were added. [Wine smoke CI 41366257752](https://github.com/sguzman/intimatr/actions/runs/41366257752) verified that the Linux-built DLL loads and shuts down inside a disposable 64-bit Wine process. It does not verify an arbitrary game, runtime attachment, or speed manipulation. **Weighted verified expansion remains 13% (previous 13%, delta +0 pp).** See the detailed roadmap for active tasks and next steps.

**2026-11-02 timing-hook checkpoint:** INT-1002 is verified by the controlled Windows/Wine QPC IAT timing proof with original-import restoration; see [timing-design.md](docs/timing-design.md). **Expansion 16% (previous 13%, delta +3 percentage points).** This credits only hook feasibility, not production hooking, Bottles or full CI on current main.

**Three-API Wine integration note (2026-11-02):** the isolated CI fixture verified QPC/GetTickCount64/GetTickCount at 0.5×/2×/1× with three imports restored ([run 41368311733](https://github.com/sguzman/intimatr/actions/runs/41368311733)). INT-1102/1103 still require production-ready hook lifecycle and real compatibility evidence. **16% verified; delta +0 pp** after this additional uncredited test.

**Four-API Wine + rollback checkpoint (2026-11-02):** run [41370469163](https://github.com/sguzman/intimatr/actions/runs/41370469163) verified four import hooks (including WinMM), 0.5×/2×/1× speed transitions, rollback fault cases 0–3, clean restoration, and shutdown in a disposable Wine test process. Native Windows smoke and concurrent reads are being tested separately. This is not yet proven inside real Bottles games or as a production-safe attachment mechanism; INT-1102/1103/1104 remain pending. **Progress 16% (previous 16%, delta +0 percentage points)**.

**First live Wine process attachment proof (2026-11-02):** [run 41371726886](https://github.com/sguzman/intimatr/actions/runs/41371726886), `wine-load-smoke` job, confirmed the Linux-built PE helper loaded `intimatr.dll` into an already-running owned Wine fixture, with target-observed Running/Stopped lifecycle. INT-1003 / INT-1407 are still open: native Bottles manager selection and real-game verification are absent. **16% verified, delta +0 pp**.

**Linux-native running-process speed control proof (2026-11-02):** [CI 41373127184](https://github.com/sguzman/intimatr/actions/runs/41373127184) succeeded with the native `intimatr-bottles probe-attach` CLI loading a Windows DLL into an already-running Wine test process, then the target measuring all four timers at 0.5×/2×/1× and restoring its imports. **Real-game/Bottles validation and production-safe attachment are not verified; 16% expansion, previous 16%, delta +0 percentage points.**

**Sequential Wine duplicate-attach prevention checkpoint (2026-11-02):** [CI 41373871724](https://github.com/sguzman/intimatr/actions/runs/41373871724) confirmed that an already-running Wine fixture received one Intimatr DLL load, then rejected a second attachment while loaded. The helper independently checks the target module list before and after the load, and the target verified timing and clean shutdown. Cross-helper races and real Bottles game attachment remain open. **16% verified; previous 16%; delta +0 percentage points.**

**2026-11-02 checkpoint:** [concurrent attach CI 41376199401](https://github.com/sguzman/intimatr/actions/runs/41376199401) passed, verifying one winning and one rejected competing attach under Wine. [Speed policy CI 41376902970](https://github.com/sguzman/intimatr/actions/runs/41376902970) passed for per-bottle/per-executable override matching; later read-only CLI preview QA also passed in [run 41377364276](https://github.com/sguzman/intimatr/actions/runs/41377364276) (other jobs still active at the checkpoint). Real native Bottles field QA, full game attachment and lifecycle remain undone. **Verified expansion 16% (previous 16%, delta +0 pp).**

**2026-11-02 PID and program matching checkpoint:** [run 41466434127](https://github.com/sguzman/intimatr/actions/runs/41466434127) completed all CI jobs for prefix-bound Linux process inspection. [Wine run 41467190463](https://github.com/sguzman/intimatr/actions/runs/41467190463) independently confirmed a Windows PID with the ToolHelp snapshot. [Linux correlation run 41468039689](https://github.com/sguzman/intimatr/actions/runs/41468039689) passed full-path registered-program fixture matching; full CI for newer code still in progress when recorded. All outputs remain read-only and non-attachable, and real Bottles field testing is missing. **Expansion 16%, previous 16%, delta +0 pp.**

**Read-only live Bottles/Wine review checkpoint (2026-11-02):** [CI 41469750714](https://github.com/sguzman/intimatr/actions/runs/41469750714) verified the native Linux `live-match` review pipeline against a real Wine process plus mocked Bottles program registration. It rejected missing bottle names and unrelated executables while keeping prefix ownership unverified and every match non-attachable. All three CI jobs passed, including native Windows DLL smoke and packaging, Linux PE/ELF builds and uploads, and the Wine live-match and timing regression fixtures. Real native Bottles validation, offline-game attachment and cleanup remain open. **Expansion 16%, previous 16%, delta +0 percentage points.**

**2026-11-02 Bottles prefix identity work:** implemented read-only lookup of the configured root via `bottles-cli info bottles-path`, fixed native name-keyed and empty JSON inventory handling, and added exact direct-child/non-symlink prefix validation in `live-match`. Automatic prefix selection now replaces manual path guessing; optional explicit prefix still must match exactly. CI verification is in progress at this checkpoint. Real native Bottles runner and game QA have not occurred. **16% verified, previous 16%, delta +0 percentage points.**

**2026-11-02 runner and PE-header inspection checkpoint:** The native `live-match` CLI now reports observed Bottles runner/architecture metadata without assuming a managed Wine binary location. Recorded Steam executable equality is separately validated, and its evidence never grants attachment. Windows PID reports now include bounded PE-file machine detection, preserved in read-only candidate matching; live CI validation is pending at this checkpoint. Actual native Bottles installation, runner, in-memory process machine and real-game proof remain unverified. **Weighted expansion: 16%, previous 16%, delta +0 percentage points.**

**2026-11-02 live process architecture checkpoint:** Implemented read-only optional Win32 `IsWow64Process2` observation in `wine-process-list.exe`, with separately preserved process-machine and on-disk PE architecture, conflict refusal and synthetic consistency tests. Wine fixture regression now asserts that any successfully queried x86-64 fixture machine agrees with its x86-64 PE file; unimplemented APIs remain unknown, not assumed. Full current CI is pending. **16% weighted expansion (previous 16%, +0 pp)**; no real native Bottles or game validation completed.

**2026-11-02 verified live machine and explicit runner gate:** [Wine CI 41481506734](https://github.com/sguzman/intimatr/actions/runs/41481506734) observed a real running x86-64 fixture machine through `IsWow64Process2` and confirmed consistent on-disk PE architecture, four speed timers, duplicate attach refusal and clean shutdown. Native `live-match` and `win-processes` were then changed to reject omitted Wine runner parameters; latest full CI for these changes remains pending. A real Bottles installation/runner and offline game have not been inspected. **16% verified; previous 16%; delta +0 percentage points.**

**2026-11-02 x64 DLL machine mismatch guard:** Read-only game matching now rejects registered x86/ARM64 candidates when both observed process and PE file machines are known to be incompatible with the current x86-64 DLL; contradictory machines also fail closed. Added unit tests and a Linux synthetic CLI fixture; latest CI pending. Real bottle/runner/game acceptance and an x86 DLL remain unverified. **Progress 16%, delta +0 percentage points.**

**2026-11-02 shared-preview integration checkpoint:** The preview-only speed request/status command now routes through the established RPC/CommandExecutor boundary, with explicit opt-in Wine-visible shared-profile loading, fail-closed game HWND selection, focused-key polling, and coordinated reset/shutdown. None of those preview controls activates the timing-hook backend. Full native Windows and isolated Wine CI for the latest RPC regression remains under review; real Bottles game enrollment, authoritative applied-speed status and UI controls remain unfinished. **Weighted progress 16% (previous 16%, delta +0 pp).**

**2026-11-02 command-to-hook fixture checkpoint:** The four-API experimental IAT clock backend now accepts **shared CommandExecutor speed presets only while its explicitly allowlisted disposable Wine fixture already owns four installed import hooks**. The Wine smoke job in [run 41500937094](https://github.com/sguzman/intimatr/actions/runs/41500937094) passed measured 0.5×/2×/1× transitions from both direct and shared-command control, invalid preset rejection, concurrency and original-import restoration. This run's format/portable verification jobs failed on formatting before [follow-up](https://github.com/sguzman/intimatr/commit/1e7b99946217935de50ad15fa4f36d6e7b359772); do not cite it as a full CI pass. Ordinary games are **not** eligible for IAT hook activation, and `clock_applied_to_game=false` remains correct for preview-only RPC status. INT-1102, INT-1103, INT-1104 and INT-1501 are still incomplete pending production-safe architecture and field QA. **Verified expansion remains 16% (previous 16%, +0 pp).**

**Standalone RPC status staging (2026-11-02):** The Windows/Wine `intimatr-speed` CLI now supports explicit loopback TCP or named-pipe speed queries and commands without a foreground scanner window. It does not find/attach games or bypass the existing RPC authorization. Initial Windows build/CLI parser CI and instrumented Bottles game field QA remain outstanding; INT-1503 is not credited. **Expansion 16% (previous 16%, +0 pp).**

**2026-11-02 speed RPC client packaging checkpoint:** The Windows build and Linux-hosted Wine cross-build CI now build and verify a standalone `intimatr-speed.exe` Windows PE client. The MSVC release package includes the CLI and a separate SHA-256 entry, both checked by the release verifier; CI publishes distinct native-Windows and Linux-cross-built Wine client artifacts. Real-game command execution and native Bottles field QA still block INT-1503, INT-1702 and INT-1404; do not award credit from packaging changes alone. **Verified expansion remains 16% (previous 16%, delta +0 pp)** pending complete CI and owner validation.

**2026-11-03 explicit timer arming and shared preset checkpoint:** The dedicated `arm-game-timers` command now requires exact registered offline-game identity and independent experimental opt-in; it never activates during ordinary attachment. `set-game-speed` requests bounded half/normal/double presets via the shared speed-command executor, and `status-game` independently reads the installed timer-session multiplier without claiming the actual game's simulation speed. [Wine CI run 41711702763](https://github.com/sguzman/intimatr/actions/runs/41711702763) passed the integration test for external 0.5×/2×/1× requests, in-process four-API clock measurement, and external stop/rollback; that run's Windows/Linux Clippy jobs failed on a complex function return type, subsequently refactored into a small named bundle in commit `bc36cb7`. Full CI for this follow-up was still in progress when this checkpoint was recorded. No actual user game or native Bottles installation was modified or tested, and `gameplay_speed_verified` remains false. **Expansion 16% (previous 16%, delta +0 pp)**: INT-1102/1103/1104/1105, INT-1403/1407 and INT-1501/1503 acceptance remain open.

**2026-11-03 configured selected-game steps and idle status checkpoint:** Commit `c03528c` carried the explicit custom 1.75× selected-game Wine profile fixture; commit `200df0e` added best-effort target-clock reset after command/backend synchronization failure. [Full CI 41741954773](https://github.com/sguzman/intimatr/actions/runs/41741954773) passed Windows Rust, Linux cross-build and Wine integration for `200df0e`. A subsequent change (`bfe5c06`, regression `f453d89`) makes authoritative installed-clock status sample QPC to expire bounded holds even when a game is idle; latest commit `82c42f6` documents this behavior. **The new idle-status change still requires CI verification; real Bottles field testing and gameplay-speed proof remain unverified. Expansion: 16% (previous 16%, Δ +0 pp).**

**2026-11-04 Bottles environment policy preflight:** The explicit registered-program profile launch now reads the selected native bottle's environment metadata and refuses a restricted variable not on its inherited allowlist, a conflicting bottle-level profile override, or unknown/malformed policy data before launching. Portable unit tests and negative mocked-native-CLI Wine regression cases were added. A separate read-only `profile-env --bottle NAME` reports whether that selected bottle's metadata permits the handoff, with a blocker reason and no game launch. `profile-launch` additionally checks the selected registered program's own per-game `environment` field, rejecting an override of the profile variable even when bottle inheritance is permitted. Wine CI includes a distinct negative mock for this case. Passing the preflight cannot prove the game received the variable. Actual Bottles field QA and real offline-game speed behavior remain unverified. **Expansion 16% (previous 16%, Δ +0 pp); latest CI pending.**

**2026-11-04 CI repair and Wine fixture reliability checkpoint:** Repaired Rust formatting in Bottles profile preflight code, restored the Linux-only module boundary for `profile_launch`, and extended the disposable Wine attachment target's initial wait from 16 to 60 seconds so slow identity/preflight fixture stages do not expire the target prematurely. These are integration test and portability fixes, not new proof of real Bottles propagation or offline-game acceleration. Current CI verification is pending. **Verified expansion remains 16% (previous 16%, Δ +0 percentage points).**

**2026-11-04 read-only game-level environment QA extension:** The no-launch `profile-env` report now optionally selects exactly one registered program by `--program` and reports bottle-wide and game-specific policy separately, including a combined decision only when a program was selected. Synthetic tests and mocked CLI Wine assertions cover a clean registered program and a conflicting per-program override. The feature does not run Wine or establish environment delivery to any real game; weighted completion remains **16% (previous 16%, Δ +0 pp)** pending CI and native-Bottles owner QA.

**2026-11-04 first native Bottles field QA and compatibility triage:** On an owner-operated native Linux Bottles installation, a single 64-bit registered offline title was verified with `profile-env --bottle ... --program ...` returning both inherited-variable policies accepted. `bottle-compat` independently verified the registered prefix and executable path but found only the `QueryPerformanceCounter` import; the other three timer names needed by the experimental four-IAT backend are absent from that executable's static imports. This is **not** an attachment or speed test: do not weaken the four-timer gate or claim a game is accelerated. Added explicit `bottle-compat --all` read-only inventory to find candidate registered titles while retaining unknown/partial/complete classifications and per-bottle inspection errors. The full CI and subsequent real-field scan remain outstanding. **Verified expansion 16% (previous 16%, Δ +0 pp).**
