# Multi-game Bottles startup proxy (offline only)

One installed Intimatr runtime binary can be reused across multiple **separate**
Bottles game directories. **Do not copy a single steam_api64.dll proxy to every
game.** Each proxy must forward *that game's own original* named Steam API
exports, including their ordinals. Games without a local, imported 64-bit
Steam API DLL need a different loading strategy.

## Safety model

- The discovery step is read-only and only considers **registered Bottles
  executables** on their selected bottle's C: drive with all four static timer
  import names. The launcher can use another timing source despite these imports,
  so compatibility is not guaranteed.
- Review the selection manifest; exclude online games, anti-cheat games, games
  you cannot modify, and untested or sensitive installations.
- The batch script limits each operation to 32 explicitly selected game
  directories and refuses duplicate directories.
- Install preflights the complete batch, then compiles a different x64
  `steam_api64.dll` proxy for every chosen game from its **actual** export
  table before modifying the first game.
- Each install checks the original Steam DLL SHA-256 immediately before
  replacement, backs it up as `steam_api64_intimatr_original.dll`, writes
  the DLL/config/ownership marker in that game's directory, and refuses
  existing unmanaged files or symlinks.
- Removal independently verifies every owned file's SHA-256 and restores the
  original DLL before deleting auxiliary files.
- No injection, attachment, Wine launch, or timer-hook activation is attempted
  by discovery, plan, install, or remove.
- Installation is **not an all-games atomic transaction**. A partial failure
  can leave earlier games successfully installed; see the individual reports
  and remove explicitly after investigating. No unrelated installations are
  silently rolled back.
- DLL proxying is **only one strategy**: games without statically imported
  `steam_api64.dll`, games with ordinal-only/unsupported exports, and games
  under non-C: mapped host drives are excluded from this first batch path.

## Command sequence

Obtain the matching `intimatr-bottles` binary, built from the current source,
and the Windows x64 `intimatr.dll` runtime. Run these commands from the
repository root, using actual absolute paths:

```bash
# 1. Produce a NEW reviewable manifest; no game is modified.
bash scripts/bottles-discover.sh /absolute/path/to/intimatr-bottles /absolute/path/to/selection.json

# 2. Edit selection.json to include only games you explicitly authorize.
# Example structure:
# {"version":1,"games":[
#   {"game_dir":"/home/user/.local/share/bottles/bottles/Example/drive_c/Games/Game","executable":"Game.exe"}
# ]}

# 3. Verify all selected games without modifying them.
bash scripts/bottles-multi-game.sh plan /absolute/path/to/selection.json /absolute/path/to/intimatr-bottles

# 4. After closing every selected game and confirming offline-only use:
bash scripts/bottles-multi-game.sh install /absolute/path/to/selection.json /absolute/path/to/intimatr-bottles /absolute/path/to/intimatr.dll --offline-single-player --games-closed

# 5. After closing every selected game, restore all selected originals:
bash scripts/bottles-multi-game.sh remove /absolute/path/to/selection.json /absolute/path/to/intimatr-bottles --offline-single-player --games-closed
```

The native `intimatr-bottles launch-proxy status --game-dir /absolute/game/dir`
command remains read-only. The established Eastward deployment and its legacy
state format remain supported.

## Launch-time speed activation is separate

The batch installer **does not** turn on acceleration. A correctly forwarded
proxy loads `intimatr.dll`, but actual timer hooks require **both**
`INTIMATR_SPEED_PROFILE` pointing to an accepted Windows-visible speed TOML
and `INTIMATR_EXPERIMENTAL_STARTUP_TIMERS=1` in the **game process**
environment. Each selected bottle must allow those inherited variables where
environment inheritance is restricted. Launch the game from a graphical session
and verify *actual gameplay speed*. Do not interpret a successful
`bottles-cli` exit as proof the game launched.

For a registered game with a separately exported, verified Windows-visible
speed profile, the native `intimatr-bottles profile-launch` command supports
an explicit `--mangohud` option to request MangoHud and HUD telemetry for
that selected invocation. This requires allowlisting the relevant environment
variables in restricted Bottles environments and does not itself install
the per-game HUD config. The `--experimental-four-timers` opt-in is separate.
The opt-in `--mangohud --managed-mangohud` finds the installed,
unchanged per-game HUD automatically from the exact registered executable.
The alternative `--mangohud --mangohud-config
/absolute/game/directory/MangoHud.conf` lets the owner select its host path
explicitly. Both routes enforce the same checksum and environment checks
before setting `MANGOHUD_CONFIGFILE` for this game only. Both can be
checked without starting Wine using `--check` instead of `--launch`. A CLI success report does not confirm an actual game process
or overlay.
See [performance HUD setup](performance-hud.md).

Adding a fully verified per-game launch shortcut and GUI Bottles integration
remains open; this bulk stage automates discovery, compilation, deployment,
inventory, and rollback only.
