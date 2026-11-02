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
