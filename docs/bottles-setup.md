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
The read-only discovery command now also records `bottle`,
`registered_program`, and `windows_executable` for each candidate, preserving
the exact native Bottles registration identity. Existing version-1 manifests
without those extra fields remain valid; the coordinator does not yet use
registration metadata to generate launch shortcuts. A future managed launcher
will validate it against the current registration rather than trusting a
possibly stale path.

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

## Persistent journal and selective rollback (draft)

Every `apply` attempt that reaches the mutation stage creates a private,
mode-0700 journal inside the canonical native Bottles root (a direct child
named `.intimatr-setup.XXXXXXXX`). The command prints its exact absolute
path. The journal retains the reviewed manifest, lists of **newly selected**
DLL/HUD changes, the prior environment ownership states, and an atomic
last-stage marker. It contains no runtime binaries. Keep the journal until
its changes are no longer needed.

```fish
# Inspect the journal path printed by apply. This is read-only.
bash scripts/bottles-setup-recover.sh status "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles"

# Explicitly undo newly managed changes, preserving pre-existing installations.
# Close Bottles and all selected offline games first.
bash scripts/bottles-setup-recover.sh rollback "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles" --offline-single-player --games-closed --bottles-closed
```

Recovery preflights the journal's path, ownership and selections, verifies
current per-component integrity, and refuses the entire rollback if any
candidate was modified, moved, or became unowned. It removes only newly
configured owned HUD files and DLL proxies, then restores environment settings
for bottles that were unmanaged before that attempt. The stage marker
survives interrupted commands, allowing the same journal to be inspected
again; it is not proof that a started stage completed. Successful rollback
marks the journal `reverted` and keeps it for inspection. Setup and
rollback serialize through an advisory lock on the native Bottles directory.
A new `apply` also refuses to proceed while an earlier attempt's journal
has an unfinished stage (including an interrupted rollback). Inspect that
journal and resolve any ownership conflicts before attempting another apply.

**Limitations:** This is an explicit, best-effort recovery path, not
cross-component atomicity, a guarantee against power loss, or protection from
Bottles/user edits made concurrently outside Intimatr. An interrupted or
partially installed file that cannot pass current ownership checks is left
untouched for manual inspection; the journal does not authorize guessing
or deleting a modified file.

## One selected-game launch (draft)

`scripts/bottles-launch.sh` now composes the existing native, ownership-checked
`profile-launch` command from the exact registration stored in a recently
reviewed discovery manifest. It does **not** install a DLL, modify an
environment allowlist, write a profile, or start an unselected game.

The source policy and delivered profile must already exist and match. This
first stage takes those two absolute paths explicitly; eliminating them through
a managed per-game configuration is still pending.

```fish
bash scripts/bottles-launch.sh check "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" "/absolute/source-policy.toml" "/absolute/delivered-profile.toml" --offline-single-player

# Replace check with launch only after reviewing the check report.
# Explicit speed-hook opt-in remains separate:
# --experimental-four-timers
```

The wrapper verifies one selected game and its unchanged native Bottles
registration, then reuses the Rust manager's source/profile/HUD/environment
preflight. It uses the owned per-game MangoHud configuration. The default
`--renderer auto` makes a choice **only** if the executable's static PE imports
unambiguously show `opengl32.dll` or `vulkan-1.dll`. Because many games
load renderers dynamically or use DXVK, ambiguous results fail closed and
require an explicit `--renderer opengl` or `--renderer vulkan`. OpenGL uses
the integrated `mangohud --dlsym` path; Vulkan uses ordinary MangoHud
activation. Neither static hints nor successful native CLI exit prove the
actual overlay rendered.

This is a safe incremental launcher, **not yet** the finished zero-configuration
per-game shortcut or complete bulk setup experience.

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
  not globally atomic. If a later component stage fails, earlier successful
  stages may remain. The durable attempt journal and the recovery command
  above permit explicit, ownership-checked selective reversal, but cannot
  guarantee recovery of incomplete or externally modified components.
- This configures a managed overlay but does **not** automatically choose an
  OpenGL vs Vulkan launch, create desktop shortcuts, export per-game profiles,
  measure startup time or verify live gameplay. Explicit
  `profile-launch --mangohud-dlsym` support is separately under PR #6.
  These integrations remain part of issue #4.
