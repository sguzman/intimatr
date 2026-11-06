# Intimatr performance HUD (Bottles / Wine)

Intimatr's HUD is intentionally split into **two independent parts**:

1. [MangoHud](https://github.com/flightlessmango/MangoHud) renders real
   frame rates, frame times, CPU/GPU activity, system RAM/VRAM, and the current
   game's process memory. Intimatr does **not** duplicate a Vulkan, DXVK or
   OpenGL renderer hook.
2. The Windows Intimatr DLL publishes the **observed four-timer hook rate** to
   `intimatr-hud-rate.txt` alongside the loaded DLL in the game directory.
   MangoHud's `exec` field reads this one-line file as the `Speed` value.

For example, the compact top-left overlay shows:

```text
INTIMATR
FPS        60
Frametime  16.7 ms
CPU        14%
GPU        48%
RAM        11.2 GiB
VRAM       3.8 GiB
Speed      2.00x
```

These are **illustrative values**, not actual game measurements.

## Installation

Install native MangoHud on Arch/EndeavourOS (no Flatpak required):

```sh
yay -S --needed mangohud
```

Build or obtain an Intimatr Windows DLL version that contains
`speed_hud_telemetry`. The older Eastward pilot DLL does not publish the speed
file and must be updated separately after the game is closed. The unchanged
`steam_api64.dll` forwarding approach still works.

Use the **same reviewed selection manifest** from the multi-game installer.
Ensure each game already has an Intimatr deployment. Without touching live
games, check the candidates:

```sh
bash scripts/bottles-hud.sh plan /absolute/selection.json
```

To inspect installed, absent, or locally modified HUD configurations without
changing anything, run:

```sh
bash scripts/bottles-hud.sh status /absolute/selection.json
```

To inspect the same selected games without starting Wine, render hooks,
or modifying files, generate an independent **JSON diagnostic report**:

```sh
bash scripts/bottles-hud.sh diagnose /absolute/selection.json
```

The report distinguishes owned, modified, absent or incomplete HUD configs,
and reports whether each `intimatr-hud-rate.txt` sample is fresh (up to six
seconds old), expired, missing, future-dated or unsafe. Stale or unsafe
samples never return a displayed speed value. It also reports whether the
host `mangohud` command is visible on `PATH`, not whether the Vulkan layer
loaded. The flags `in_game_rendering_verified` and
`gameplay_speed_verified` always remain `false`: neither a host-side
timestamp nor a configuration file can establish those facts.

Then, with those selected games closed:

```sh
bash scripts/bottles-hud.sh install /absolute/selection.json --offline-single-player --games-closed
```

This creates only `MangoHud.conf` and an ownership SHA-256 marker beside
each selected game executable. If a later selected game cannot be configured,
the installer rolls back any unchanged HUD files it created earlier in the
same batch; files modified by another process are preserved. It refuses
existing configs rather than
overwriting user's prior MangoHud settings. The config is specific to that
game's install directory, so it does not change global HUD preferences or
affect other games. MangoHud normally discovers a `MangoHud.conf` beside the
application; an explicit `MANGOHUD_CONFIGFILE` or `MANGOHUD_CONFIG` override
can take precedence, so check those variables if the per-game layout is
not showing.

For an individually registered offline game, the native `intimatr-bottles
profile-launch` command accepts the additional `--mangohud` flag.
This passes `MANGOHUD=1` and `INTIMATR_HUD_TELEMETRY=1` **only to the
selected Bottles launch**, after requiring both variables to be allowed by
the selected bottle and not overridden by the selected registered program.
Use it with the existing required `--config`, `--bottle`, `--exe`,
`--program`, `--profile`, `--offline-single-player`, and `--launch`
arguments. It does **not** arm timer hooks unless the independent
`--experimental-four-timers` acknowledgment is also supplied.

For deterministic use of the installed **Intimatr per-game layout**, add
`--mangohud-config /absolute/game/directory/MangoHud.conf` alongside
`--mangohud`. The manager verifies the exact registered executable's
directory, the unchanged Intimatr ownership SHA-256 marker, and the selected
bottle/program environment permissions for `MANGOHUD_CONFIGFILE`. It clears
any inherited host `MANGOHUD_CONFIG` override for this invocation and
rejects explicit conflicting Bottles/program values. A registered game on a
non-C: Wine drive is resolved only for this explicit selection through that
bottle's verified drive mapping. This must follow the separate HUD install.

Before launching a selected game, replace `--launch` with `--check` on
the exact same `profile-launch` command (keeping all other selection,
profile, and HUD flags). This performs the full read-only game/config/
environment preflight and returns a JSON report with
`"bottles_run_invoked": false`. It verifies *readiness*, not MangoHud
rendering or gameplay speed. The options are mutually exclusive;
`--check --launch` is rejected.

Alternatively, enable MangoHud for the selected bottle using its **Monitor
Performance / MangoHud** setting, or launch the Vulkan game with the
`MANGOHUD=1` environment variable. If Bottles restricts host environment variables, `MANGOHUD`
and `INTIMATR_HUD_TELEMETRY` must be explicitly permitted in its inherited
environment for `--mangohud`. OpenGL can need
the `mangohud` command wrapper; plain `MANGOHUD=1` is only guaranteed for
Vulkan. If running under Gamescope, use its own supported MangoHud integration.
A successful `profile-launch --mangohud` report means Bottles accepted the
launch command, **not** that the game started, MangoHud rendered, or the
telemetry reached the game. Existing `MANGOHUD_CONFIG` or
`MANGOHUD_CONFIGFILE` overrides may supersede the installed game-local
layout and require separate verification.

For speed telemetry, launch with both:

```text
INTIMATR_SPEED_PROFILE=C:\path\to\enabled-speed.toml
INTIMATR_EXPERIMENTAL_STARTUP_TIMERS=1
```

The existing explicit startup-hook opt-in also enables the rate exporter;
there is no extra Bottles environment variable for it. The exporter can be
enabled independently with `INTIMATR_HUD_TELEMETRY=1`, in which case no hooks
are armed and Speed shows `--` when unavailable. A displayed `2.00x`
is the **installed virtual clock's** rate, *not* a claim that every game
engine runs twice as fast. Real gameplay acceleration must be observed.

## Controls and safety

- **Right Shift + F12** toggles the HUD, independently of speed.
- `Ctrl+F7`, `Ctrl+F8`, `Ctrl+F9`, `Ctrl+F11` remain the speed controls.
- `--` means no verified, installed four-timer hook session. The HUD also displays `--` before the telemetry file exists, rather than leaving the value blank.
- The HUD exporter is read-only: it polls an existing hook state and writes
  a short, plain-text status file, never installs hooks, reads game memory
  or adds network services.
- While active, the exporter refreshes its rate file at least every 2 seconds
  even when the speed multiplier is unchanged. MangoHud reports `--` if the
  rate file is missing, empty, dated in the future, or more than 6 seconds old.
  A clean shutdown writes `--`; an abrupt crash is therefore reflected after
  the freshness window. This remains a *best-effort heartbeat*, not proof of
  game health, actual rendering, or measured gameplay speed.
- MangoHud's upstream `exec` integration runs a shell; that path has known
  compatibility limitations with some launchers. If it stalls a particular
  game, remove the optional HUD using the independently verified rollback
  command. Do not assume the render hook is universally compatible.
- CPU/GPU percentages and RAM/VRAM figures are MangoHud measurements; some
  depend on available drivers and sensors. Per-process memory (`procmem`)
  is a different quantity from total system RAM.

Uninstall **only** Intimatr-managed HUD configs with:

```sh
bash scripts/bottles-hud.sh remove /absolute/selection.json --offline-single-player --games-closed
```

If `MangoHud.conf` has been modified since the script created it,
removal refuses to delete it. The Intimatr DLL and its original-DLL backup
are untouched by HUD removal. An unchanged, Intimatr-owned HUD can also be
removed *after* the managed DLL proxy has been removed; the HUD cleanup does
not depend on the proxy still being installed.
