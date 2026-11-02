# Virtual-time and Windows timer hook design

**State:** preliminary engineering specification for Milestone 10 (INT-1002). The synthetic virtual-clock core is at [src/speed.rs](../src/speed.rs); Windows timing hooks are **not implemented** and Wine behavior is **not yet field verified**.

## Requirements

A game reading elapsed time from a supported Windows timing API should observe a strictly nondecreasing virtual clock. Speed transitions must not jump backward or forward. Setting 0.5× should halve the apparent elapsed rate, 1× preserve it, and 2× double it within the supported timer's resolution. Default disabled/incompatible operation must leave calls untouched.

These goals are not the same as faster actual hardware execution, increased frame rate, bypassing frame caps, or changing the scheduler. Some game logic advances through waits, simulation tick counts, multiplayer servers or unsupported timers. Such applications may ignore this mechanism or behave inconsistently and must be reported as incompatible rather than promised universal speed control.

## Selected v1 API candidates and deliberate exclusions

| Windows API | Desired behavior | Notes |
| --- | --- | --- |
| `QueryPerformanceCounter` | Return a scaled monotonic performance counter | Preserve the real counter frequency from `QueryPerformanceFrequency`; hook only the counter read |
| `GetTickCount64` | Return a scaled monotonic millisecond count | Shared speed state; conversion from the reference clock requires explicitly chosen units and anchoring |
| `GetTickCount` | Return the lower 32 bits of a consistent millisecond clock | 32-bit wrap is expected; do not promise the raw 32-bit value itself never wraps |
| `timeGetTime` | Return a millisecond counter consistent with GetTickCount | May require a Windows-compatible winmm import/proxy path and verification under Wine |

`QueryPerformanceFrequency` should not be scaled: changing frequency changes conversion semantics and confuses different modules. `Sleep`, `NtDelayExecution`, blocking waits, graphics timers, wall-clock `GetSystemTimeAsFileTime`, and arbitrary engine-specific timers are **not** v1 targets. Broaden the hook set only with evidence of a supported game's unmet requirements and renewed verification.

Actual interception needs to be selected after comparing a narrow in-process detour vs module-scoped import-table redirection. For a game depending on delayed DLL imports or issuing calls from different modules, one method may miss calls. Do not write arbitrary game memory before validating page protection and original function/trampoline behavior; keep all unsafe logic in a small Windows-specific module. `DllMain` remains loader-lock safe; hooks only activate after Intimatr bootstrap is ready. Uninstall/stop must not leave live call sites pointing at freed code.

## Virtual clock invariant and synthetic proof

The platform-neutral model stores a real-time anchor, virtual-time anchor, last-real tick, last-virtual tick, and multiplier. It accepts caller-supplied ticks with a monotonic unit domain and clamps out-of-order readings. On a speed change it samples under the old multiplier, reanchors, and switches to the new multiplier. Invalid (zero, negative, NaN, infinite) values are rejected before altering state. Saturation prevents integer overflow on extreme durations.

The synthetic tests in `src/speed.rs` cover: 1× identity, 0.25× and 2× progression, reset to 1×, no discontinuity at repeated transitions, invalid-rate rollback, out-of-order sampling, and saturation. The engine currently uses `f64` for elapsed scaling and floors fractional ticks. For extremely large tick values, precision is limited by floating-point representation and saturation; documented limitations must be verified before using these rates with real QPC frequencies.

A **single clock instance cannot accept raw ticks from different APIs with incompatible epoch/frequency**. The new platform-neutral [WindowsTimers](../src/speed_timers.rs) projection uses the real QPC as the only input clock; its virtual QPC delta is converted to millisecond domains using the unchanged QPF. It retains separate base values for GetTickCount64 and timeGetTime and deliberately preserves 32-bit wrapping. [SpeedSession](../src/speed_session.rs) coordinates profile preset transitions with that same clock, instead of maintaining disconnected UI and timer state. The hook backend still needs to acquire the true native QPC once per intercepted call and initialize the native epochs. Tests must compare QPC, milliseconds and wraparound under rate changes; no implementation is accepted for live Wine games until multi-API consistency is established.

## Concurrency, safety and performance acceptance

- All hooks must read one logically consistent speed update; use a synchronization strategy which avoids indefinite blocking or recursion inside timing functions. The current `VirtualClock` is not internally synchronized; its caller owns a mutex or equivalent while prototyping. Final hook fast paths need documented contention/latency behavior.
- Preserve the original API path and return types; failure to install one requested hook must be observable. Either fail atomically to unmodified mode or give an explicit documented partial capability that is checked by the CLI/UI.
- Dynamic profile changes, reset, game exit, repeated attach and failed detach must be handled without corrupting the target process. Attach only to selected offline games; no anti-cheat bypass.
- Per-API real-OS test harness should log raw and scaled deltas against a reference monotonic source, and calculate rates for 0.5×, 1× and 2× with a tolerance based on timestamp resolution. Existing scanner/debugger/RPC behavior remains a required regression gate.
- Track when the Wine runner's implementation deviates from native Windows. CI cross compilation alone proves neither Wine hook interoperability nor correct game speed.

## Explicit research outcome

- **Chosen initial clock model:** scaled elapsed duration with reanchoring, using the pure Rust virtual clock, not direct fixed offset jumps.
- **Chosen initial API scope:** QueryPerformanceCounter first, followed by GetTickCount/64 and timeGetTime once coordinated hooks and regression coverage exist. **Import-address-table replacement** is the first proven mechanism for a linked 64-bit PE executable; inline detours remain a later alternative for late-bound and non-imported calls.
- **Feasibility demonstrated so far:** synthetic clock tests plus the isolated 64-bit Wine QPC IAT interception probe described below. Production game hooks, late-loaded modules, timer consistency across real APIs, concurrent-call teardown, and running-process attachment are **not** verified.
- **Next engineering steps:** run deterministic tests, select hook method via isolated Windows/Wine harness, and prove the same rates under a selected offline game.

External documentation: [Microsoft on Windows DLL runtime loading](https://learn.microsoft.com/en-us/windows/desktop/Dlls/run-time-dynamic-linking), [Rust saturating float-to-integer conversions](https://doc.rust-lang.org/rust-by-example/types/cast.html), [Wine native/builtin DLL override semantics](https://manpages.debian.org/unstable/libwine/wine64.1.en.html).

## 2026-11-01 incremental implementation checkpoint

The pure [src/speed_timers.rs](../src/speed_timers.rs) projection and [src/speed_session.rs](../src/speed_session.rs) action/session boundary have source-level synthetic tests for cross-API scaling, wraparound, continuity, invalid rates and preset transitions. These are **not hook installations**. Their acceptance depends on the next CI run and they do not close INT-1102/INT-1103 or INT-1002 until real Windows/Wine timer behavior is verified.

## 2026-11-02 — Opt-in QPC interception proof

[Experimental probe](../src/experimental_timer_probe.rs) patches **only the QueryPerformanceCounter import** in Intimatr's own `wine-load-probe.exe` test executable, and only after explicitly calling `intimatr_timer_probe_enable`. It reads raw QPC through the DLL's separate, untouched import, feeds [SpeedSession](../src/speed_session.rs), and atomically replaces one PE32+ IAT pointer with guarded memory protection changes. No Windows timer hooks are installed during regular Intimatr bootstrap, and the probe rejects executables other than `wine-load-probe.exe`.

The Linux-built Windows executable uses the ordinary imported QPC call and a dynamically resolved **unhooked** QPC call to measure apparent vs real elapsed clock rate. The [Wine smoke step in GitHub Actions run 41367449695](https://github.com/sguzman/intimatr/actions/runs/41367449695) passed, reporting:

| Requested | Measured virtual QPC elapsed / unhooked elapsed |
| --- | ---: |
| 0.5× | 0.500× |
| 2.0× | 2.000× |
| 1.0× | 1.000× |

The test also confirmed exactly **one IAT slot** patched, all imports restored, and an orderly DLL shutdown. The run's other jobs failed strict style/Clippy checks in the new code; these issues were corrected in later commits, pending passing full CI at the latest HEAD. This is a narrow **compatibility proof**, not a verified general-purpose hook backend.

### Explicit limits and safety follow-ups

- The QPC interception covers calls imported directly by the fixture's main executable. It does not capture `GetProcAddress` calls, modules loaded later, middleware DLLs, other timer APIs, or engine-specific time sources.
- The minimal experimental IAT patcher has no proven live-game reentrancy, threading, contention, unload race, or partial `VirtualProtect` failure recovery. **Never expand the executable allowlist** or turn it on automatically until those invariants are tested and all restore paths are fail-safe.
- A failed memory-protection restore can leave a patched slot; the production design must record ownership before mutation and retain the DLL until every slot is safely restored. Exact atomic rollback, page-protection transitions, and module lifetime tracking are required for INT-1102/1104.
- Speed-rate changes through exported test methods are not yet wired to the existing Intimatr CommandExecutor, egui, configuration loader, RPC or hotkeys. The normal Windows DLL remains unchanged by default.
- CI's disposable Wine prefix does not represent the user's native Bottles runner or hardware. No anti-cheat/protected target integration is planned.

**Architectural decision for INT-1002:** validate QPC via IAT in a controlled x64 PE fixture before extending to additional timer APIs; prefer a reversible per-import patcher over unvalidated broad process detours at this stage. The Windows clock projection already handles QPC-to-milliseconds unit mapping in pure Rust, but actual multi-API hook consistency remains open.

## 2026-11-02 — Coordinated three-API Wine proof

The **same disposable Windows PE probe** was extended to import `GetTickCount64` and `GetTickCount`, while retaining `QueryPerformanceCounter`. [src/experimental_timer_probe.rs](../src/experimental_timer_probe.rs) now patches those three direct imports in the controlled test executable only, using one [SpeedSession](../src/speed_session.rs) and the QPC-derived [WindowsTimers](../src/speed_timers.rs) projector. Raw API baselines are resolved separately with `GetProcAddress` and remain unpatched.

[Linux/Wine CI run 41368311733](https://github.com/sguzman/intimatr/actions/runs/41368311733) **passed both Linux cross compilation and the isolated Wine runtime test**, recording:

| Requested speed | QPC observed | GetTickCount64 observed | GetTickCount observed |
| --- | ---: | ---: | ---: |
| 0.5× | 0.500× | 0.500× | 0.500× |
| 2× | 2.000× | 2.006× | 2.006× |
| 1× | 1.000× | 1.000× | 1.000× |

The probe confirmed **three patched IAT slots**, restored all originals, and shut down cleanly. This is a real coordinated clock proof *in an opt-in, single-purpose Wine fixture*. It is not a general-purpose, thread-safe game hook. In particular, `timeGetTime`, late-loaded modules, Win32 `GetProcAddress` paths, synchronization under heavy contention and safe unload during concurrent calls remain unverified. No automatic Bottles deployment, native Linux running-process attachment or game compatibility claim follows from these results. INT-1102/1103 remain **pending/active**, not verified complete; the weighted expansion score remains **16%** until their whole acceptance criteria are satisfied.

## 2026-11-02 — Four-API and rollback verification

The test-only import patcher now covers all four planned v1 timing APIs: `QueryPerformanceCounter`, `GetTickCount64`, `GetTickCount` and the WinMM `timeGetTime`, resolved independently from the target's unhooked functions. All remain restricted to Intimatr's own **opt-in isolated test EXE**.

[Wine CI run 41370469163](https://github.com/sguzman/intimatr/actions/runs/41370469163) passed the isolated 64-bit Wine test and measured the following ratios relative to the original unhooked timers:

| Requested rate | QPC | Tick64 | Tick32 | WinMM |
| --- | ---: | ---: | ---: | ---: |
| 0.5× | 0.500× | 0.503× | 0.503× | 0.503× |
| 2× | 2.000× | 1.994× | 1.994× | 1.994× |
| 1× | 1.000× | 1.006× | 1.006× | 1.006× |

The same Wine test deliberately failed installation after 0, 1, 2 and 3 pointer replacements; every partial installation was rolled back without leaving active hook slots. The probe then confirmed four active hooks, rejected an invalid preset, exercised rate transitions, restored all imports and shut down cleanly. An earlier [rollback baseline run 41370188133](https://github.com/sguzman/intimatr/actions/runs/41370188133) independently passed the three-API version of this test plus native/Linux build checks.

**Lifecycle hardening implemented:** `HookSlot` ownership records each successful pointer swap separately from page-protection recovery. On a failed installation, restoration is retried without forgetting a remaining hook; the test executable's module guard attempts cleanup before unload and deliberately retains the DLL if any active callback remains. This is *not* complete production-grade lifecycle safety: restoration of original page permissions after an unexpected failure still needs robust verification, as do simultaneous patch/unload operations and external process exit.

**Further work in progress:** a monotonic snapshot fallback during concurrent timer reads, a multithread rate-transition stress test and a parallel native Windows MSVC smoke run. These additions are committed but must pass CI before their results are claimed. The current implementation does not load into ordinary Bottles games or running Wine processes, and no global speed control is enabled. INT-1102/1103/1104 remain unfinished; no new weighted points are awarded.

## 2026-11-02 — Four virtual clocks after live Wine-process attachment

[CI run 41373127184](https://github.com/sguzman/intimatr/actions/runs/41373127184) verified a second integration path beyond direct `LoadLibraryW` in an already-started test process: Linux native `intimatr-bottles` invoked a Windows helper via the same Wine prefix, the remote target independently observed Intimatr Running, enabled its four imported timer hooks, then measured 0.5×/2×/1× against original QPC, tick64, tick32, and winmm APIs. Exact observed ratios are documented in [Linux/Wine build and attachment](linux-wine-build.md). The target restored its original import addresses before an orderly shutdown. This proves hooks can start *after* the Windows process does, but only for Intimatr's allowlisted isolated fixture.

The existing direct-load fixture also passed four-thread/15,000-iteration sampling and 30 speed updates without backwards readings in [CI run 41370928911](https://github.com/sguzman/intimatr/actions/runs/41370928911). The remote target's speed test is currently single-threaded; safe concurrent live-game unload, late imports and unrelated game timing APIs remain explicit open risks.

### 2026-11-02: shared-command-to-Wine-hook proof (fixture only)

The experimental four-import IAT patcher now has a test-only exported function, `intimatr_timer_probe_shared_rate`, which first requires an allowlisted fixture executable and four **already installed** import hooks. It routes the selected preset through the **same CommandExecutor/SpeedPreviewExecutor used by RPC**, then updates the real QPC-anchored hook session through `set_probe_rate`. The DLL never installs an import hook on account of this command; a denied/invalid preset leaves the hook unchanged. An unexpected failure applying to the hook resets the shared preview to 1×. The two models have separate timer epochs but one authoritative speed command selection; they are **not** a production real-game timing backend.

The disposable `wine-load-probe.exe` creates a unique temporary versioned TOML profile and sets `INTIMATR_SPEED_PROFILE` before loading `intimatr.dll`. After the existing fail-injection/restore checks, the fixture measures 0.5×, 2× and 1× against unchanged Windows timer entrypoints through both direct and shared-command setters, and then restores all four imports. This proves the new RPC-style command abstraction can drive the existing isolated hook implementation; it does **not** establish compatibility with unrelated game engines. The `wine-load-smoke` job of [run 41500937094](https://github.com/sguzman/intimatr/actions/runs/41500937094) passed this sequence. Its other jobs failed the Rust formatter, fixed in a later commit; check final main CI separately before treating the revision as fully green. Real Bottles, game-window focus, reliable production cleanup and non-fixture hook enablement remain open.

### 2026-11-02: ordinary shared command synchronizes the allowlisted IAT fixture

The experimental fixture-only IAT backend now exposes an internal `synchronize_fixture_command` seam. Every status/action/input sample on the ordinary `SpeedPreviewExecutor` calls this seam, but it returns without changing anything except in the **two allowlisted disposable Wine probes** with an already installed, complete four-import hook set. It never installs hooks, never allows an unrelated executable, and refuses a poisoned/partial fixture hook state. A synchronization failure surfaces as a speed-command error and resets the preview to 1× rather than silently reporting a successful command. The two clock instances still have separate epochs; only selected rate is synchronized. This is staging toward a genuinely unified backend, not an applied-game-clock claim. The revised Wine smoke fixture directly measures actual Windows API time after issuing a normal client RPC speed command instead of invoking the separate test-only export. All new tests require CI success and do not complete INT-1102/1103 or 1501 until production-safe generalized backend and real game QA.

### Timed actions remain timed in the isolated shared-command fixture

The shared speed wrapper now forwards **the actual action** to an already-installed allowlisted probe clock. Read-only `Status` queries no longer replay a `SelectPreset`, which otherwise resets temporary hold/startup expiry. `HoldStart` and `HoldRelease` preserve their countdown on the hooked clock, which processes expiry in intercepted timer reads even without further RPC commands. Focused keyboard polling only forwards a multiplier when its effective speed changes or focus is lost, avoiding repeated timer epoch resets. The Windows/Wine `intimatr-speed.exe` CLI accepts explicit `hold MULTIPLIER DURATION_MS` and `release` actions for fixture QA. This remains a **fixture-only** backend, not an authorization to hook an arbitrary game.

### Focused hotkey parity with timed RPC commands (2026-11-02)

The portable `SpeedCoordinator` now reports its **applied input actions**, not only the resulting instantaneous multiplier. The existing `input_sample` method remains available for callers that only need its timer snapshot; the new `input_sample_with_actions` returns the same snapshot plus the ordered actions actually dispatched by the focus-aware key pump. The experimental, already-installed fixture backend receives `HoldStart { multiplier, duration_ms }`, `HoldRelease`, `FocusLost`, and other actions unchanged. Idle polling never turns a temporary hold into a permanent preset; the fixture's timer callbacks independently expire the bounded hold deadline. On a backend action error the preview is reset and the fixture receives a best-effort focus-loss reset; errors are still reported. Portable regression tests cover hold start, idle, release, focus loss and timeout. **No real Bottles game enabled and no production hook permission granted; verified expansion remains 16% (+0 pp) pending full CI and field QA.**

### Bounded timer-callback quiescence gate (2026-11-02)

The experimental IAT hook now counts every callback entered through QPC, GetTickCount64, GetTickCount and WinMM using a small RAII tracker in `speed_hook_lifecycle.rs`. After restoring every import slot, shutdown requires the counted callbacks to drain within 500 ms; otherwise it fails closed and retains the DLL's hook ownership for a later retry. Deterministic unit tests assert zero/nonzero tracking and multi-thread release behavior. **Important limitation:** a thread can read an old function pointer before an import slot is restored but enter that callback later; in-flight counting alone cannot prevent this race. This is incremental defensive hardening of a fixture-only hook, **not proof of production-safe unloading** in arbitrary Bottles games. Explicit module pinning, dispatch rendezvous or similarly reliable protection remains a prerequisite to generalize hook activation. Weighted expansion remains 16/100 (+0 pp) pending CI and field acceptance.

**Concurrent hook-restoration smoke:** Wine fixture workers repeatedly call the four imported timers while the probe restores the IAT slots. All workers are joined before DLL unloading; the test checks a successful drain and zero installed hooks, then exercises repeat disable and DLL shutdown as before. This specifically tests restore under load, not the unresolved stale pre-fetched function-pointer entry race. No real game timing hooks are enabled.
