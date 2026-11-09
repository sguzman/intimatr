# Combined Bottles setup (first CLI coordinator)

This source-checkout script coordinates the existing Intimatr environment
allowlist manager, x64 DLL/Steam forwarder installer and managed MangoHud
config installer for **one reviewed selection** of offline Bottles games.

The original component-specific checksums, backups, refusal of unmanaged
files and safety checks remain in place. This is not a universal arbitrary
DLL injector, and it never launches a game.

## Reviewed selection

Use the existing read-only `scripts/bottles-discover.sh` with the current
Cargo-installed `intimatr-bottles` binary to generate a new candidate manifest.
Review every candidate and exclude online, anti-cheat or unsupported games.

Run from the existing Intimatr source checkout with real absolute file paths:

```fish
bash scripts/bottles-setup.sh plan "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" --all-bottles

# Close Bottles and every selected game before applying.
bash scripts/bottles-setup.sh apply "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" --runtime "/absolute/cross-built/intimatr.dll" --offline-single-player --games-closed --bottles-closed --all-bottles

bash scripts/bottles-setup.sh status "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" --all-bottles
```

With `--all-bottles`, only the **environment variable allowlist** is applied
to all native Bottles. DLL and HUD installation remain explicitly scoped to
the reviewed game selection. Without it, inherited variables are limited to
the bottles containing selected games. Reapplying skips unchanged managed
DLL and HUD files rather than overwriting them.

An empty reviewed manifest is accepted for `--all-bottles` environment-only
management. If no new DLL deployment is needed, `--runtime` can be omitted.

## Limitations

- The coordinator requires the new `bottle-env` command from PR #5 and reuses
  the existing `bottles-hud.sh` and `bottles-multi-game.sh` installers.
- Read-only `plan`/`status` preflight exact files and existing ownership.
- The reviewed manifest is snapshotted once and the validated copy is used
  across all stages, so another process changing the source manifest mid-run
  cannot silently switch the selected games.
- `apply` requires affirmative acknowledgement that all selected games and
  Bottles are closed and games are offline single-player. These are
  acknowledgements, not proof that a process has exited.
- Only native C: installations are accepted. Symlinked, moved, and unmapped
  locations fail closed until their identity is supported and verified.
- For a new DLL deployment, `apply` validates the required MinGW tools,
  forwarder sources, and a readable x64 Windows PE runtime **before** changing
  any bottle environment allowlist. Read-only `plan` does not require a runtime.
- A batch is **not globally atomic**. A failure inside the environment
  batch now attempts to restore only environment edits made in that batch;
  a failure later in the DLL batch attempts to restore only newly installed,
  checksum-owned and unchanged proxies; the HUD installer similarly rolls
  back its own newly installed files. These are best-effort compensations,
  not a durable transaction or crash recovery mechanism. If a later
  component stage fails, earlier successful stages may remain. The failed
  stage is reported; existing `bottle-env restore`,
  `bottles-multi-game.sh remove`, and `bottles-hud.sh remove` provide separate
  ownership-checked reversal. A combined transaction journal is unfinished.
- This configures a managed overlay but does **not** automatically choose an
  OpenGL vs Vulkan launch, create desktop shortcuts, export per-game profiles,
  measure startup time or verify live gameplay. Explicit
  `profile-launch --mangohud-dlsym` support is separately under PR #6.
  These integrations remain part of issue #4.
