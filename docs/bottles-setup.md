# Combined Bottles setup (first CLI coordinator)

This source-checkout script coordinates the existing Intimatr environment
allowlist manager, x64 DLL/Steam forwarder installer and managed MangoHud
config installer for **one reviewed selection** of offline Bottles games.

The original component-specific checksums, backups, refusal of unmanaged
files and safety checks remain in place. This is not a universal arbitrary
DLL injector, and it never launches a game.

## One reviewed plan/apply/status (draft integration)

`scripts/bottles-full-setup.sh` is the new top-level CLI entrypoint.
It uses the **same frozen, reviewed manifest** to coordinate Bottles
environment allowlists, selected DLL proxy/HUD installation, and selected
per-game launch profiles. It performs a complete read-only profile and
base-component preflight before invoking either mutating installer.

```fish
# Review and explicitly exclude online or unsupported game entries first.
# This previews every component without modifying any Bottles/game files.
bash scripts/bottles-full-setup.sh plan "/absolute/reviewed-selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/source-policy.toml"

# Explicit opt-in: all selected offline games and Bottles must be closed.
# Supply --runtime PATH only when one of the selected games needs a new DLL.
bash scripts/bottles-full-setup.sh apply "/absolute/reviewed-selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/source-policy.toml" --runtime "/absolute/x64/intimatr.dll" --offline-single-player --games-closed --bottles-closed

bash scripts/bottles-full-setup.sh status "/absolute/reviewed-selection.json" "$HOME/.cargo/bin/intimatr-bottles"
```

The discovery manifest supplies exact native registrations and an initial
`"renderer": "auto"` for every candidate. Explicitly review renderer
choices and change ambiguous games to `opengl` or `vulkan`, after confirming
the game's actual graphics backend. `--all-bottles` remains an optional
**environment-only** expansion; it does not authorize extra games for
DLL/HUD/profile management. An empty manifest is not sufficient for this
full profile workflow.

**Failure and recovery:** The top-level command is not globally
atomic. It reports the failing stage and prints the base setup's private
recovery journal. After a successful base apply, it also records which
reviewed game profiles were already managed before the full attempt. A
separate checksum guards this before-state. After every selected
profile has been prepared and ownership-verified, a second checksummed
inventory records the exact managed-profile ownership hash and filesystem
device/inode identity observed at completion. The stage only becomes
`profiles-complete` once that inventory is durable. This allows
explicit, selective recovery of newly created profiles without deleting
pre-existing profiles **or a later replacement with matching contents**.
A new combined apply refuses an earlier unfinished combined profile stage.

```fish
# Read-only status of the same journal printed by the combined apply.
bash scripts/bottles-full-recover.sh status "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles"

# Explicit rollback, after closing all selected offline games and Bottles.
bash scripts/bottles-full-recover.sh rollback "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles" --offline-single-player --games-closed --bottles-closed
```

The recovery command checks the original base journal, both
checksummed profile inventories, the recorded directory identity,
all currently managed profile checksums, and the base component ownership
guard *before* deleting any new profiles. A recreated or changed profile
whose directory identity or owned checksum differs from the completed
snapshot blocks rollback, even if it remains a valid Intimatr profile.
Recovery then removes only unchanged profiles absent before this attempt and delegates
DLL/HUD/environment rollback to the existing guarded base command. A
profile modified by the user blocks the combined rollback; a profile
preparation stage merely marked `profiles-started` also blocks automatic
removal because its provenance is incomplete. If rollback is interrupted,
a `rollback-started` marker allows an integrity-checked retry.

The combined `apply` and `rollback` commands now hold an exclusive
advisory lock across their **full** environment/DLL/HUD and per-game
profile stages. The first explicitly approved operation creates an inert,
owner-only (0600) `.intimatr-full-operation.lock` file in the native
Bottles root. Later operations reuse this file; a symlink, foreign owner,
extra hardlink, or unsafe mode blocks mutation. The lock file remains
after setup or rollback for future serialization. `plan` and `status`
do not create or require this lock. This separate lock coexists with the
base installer's own root-directory lock, avoiding recursive deadlock.

**Limits:** The lock serializes *combined Intimatr operations* only; it
does not block standalone base operations, Bottles itself, or arbitrary
external tools. The combined operation and its rollback are still
compensating, not fully atomic or guaranteed across power loss, changes
by external tools, or every possible interruption window.
Filesystem identity is a conservative replacement guard, not a guarantee
against deliberately forged ownership or inode reuse. An interruption
after base apply but before the combined before-state is written may
require using the base recovery command directly. If the completed
after-state is missing, profile recovery deliberately fails closed.
A failed mid-profile export may leave an incomplete
profile directory for manual inspection. No Wine/game launch or desktop
shortcut installation occurs automatically.

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
last-stage marker. A fixed-order SHA-256 inventory guards the reviewed
selection, initial environment ownership, and metadata against accidental
editing; recovery refuses any checksum mismatch. Stage markers remain
mutable and are not included in that checksum inventory. The journal
contains no runtime binaries. Keep it until its changes are no longer needed.

```fish
# Inspect the journal path printed by apply. This is read-only.
bash scripts/bottles-setup-recover.sh status "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles"

# Explicitly undo newly managed changes, preserving pre-existing installations.
# Close Bottles and all selected offline games first.
bash scripts/bottles-setup-recover.sh rollback "/absolute/bottles-root/.intimatr-setup.XXXXXXXX" "$HOME/.cargo/bin/intimatr-bottles" --offline-single-player --games-closed --bottles-closed
```

Recovery preflights the journal's path, ownership and selections, verifies
current per-component integrity, and refuses the entire rollback if any
candidate was modified, moved, or became unowned. Importantly, a stage
marked `started` does **not** prove that the resources now present were
installed by that attempt: recovery blocks removal of any such resource
until the corresponding `complete` stage was recorded. Stages not yet
started likewise cannot claim ownership of files found afterward. It removes only newly
configured owned HUD files and DLL proxies, then restores environment settings
for bottles that were unmanaged before that attempt. The stage marker
survives interrupted commands, allowing the same journal to be inspected
again; it is not proof that a started stage completed. If rollback itself
is interrupted, a separate `rollback-source-stage` marker preserves the
last setup boundary for conservative retry; older incomplete rollback
journals without this marker require manual inspection. Successful rollback
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

The low-level wrapper requires the source policy and delivered profile
to be provided explicitly. The managed setup workflow below can prepare
an owned profile once and reuse it without those repeated arguments.

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

## Prepared game profiles and short launches (draft)

`scripts/bottles-managed-launch.sh` uses one reviewed game registration,
the existing `intimatr-bottles profile-export` command, and a source speed
policy to prepare a **private, Intimatr-owned** launch folder inside the
selected game directory. It does not install a DLL, configure HUD, start a
game, or change a global Bottles setting.

```fish
# Read-only preview. Use exact paths from the reviewed manifest.
bash scripts/bottles-managed-launch.sh plan "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" "/absolute/source-policy.toml" --renderer opengl

# One-time preparation, with the selected game closed.
bash scripts/bottles-managed-launch.sh prepare "/absolute/selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" "/absolute/source-policy.toml" --renderer opengl --offline-single-player --games-closed

# Later: no source/profile/manager/registration arguments to reenter.
bash scripts/bottles-managed-launch.sh status "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME"
bash scripts/bottles-managed-launch.sh check "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" --offline-single-player
bash scripts/bottles-managed-launch.sh launch "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" --offline-single-player

# Experimental timer hooks are always a separate per-launch opt-in:
# --experimental-four-timers

# Explicit owned-profile removal when the game is closed.
bash scripts/bottles-managed-launch.sh remove "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" --games-closed
```

Preparation creates only the previously absent
`.intimatr-managed-launch/speed.toml`, `launch.json`, and
`snapshots.sha256`. The directory is private (0700). The Rust manager
creates a DLL-readable, selected-game profile and confirms that speed is
enabled before the launch descriptor is accepted. A fixed SHA-256 inventory
covers both profile and descriptor. The launcher verifies intact ownership,
that the source policy has not changed, the current native Bottles
registration, the selected C: game executable, and the existing HUD/profile
preflight before it invokes Wine. A repeated identical `prepare` is a
no-op. Changed policies, game identity, renderer, local edits, and unmanaged
files are never silently overwritten.

`remove` deletes only the three exact, unchanged managed files after an
explicit games-closed acknowledgment, even if the original source policy
has moved or changed. Extra files and modified owned files block removal.
If preparation is interrupted, the incomplete directory is retained for
manual inspection, not automatically deleted. It is **not** a substitute for
the separate combined setup journal, which currently covers environments,
DLL proxies, and MangoHud, not prepared launch profiles.

Renderer `auto` remains a conservative static PE-import hint. Explicit
`--renderer opengl` or `--renderer vulkan` may be chosen at preparation.
An actual game start, custom HUD rendering, timer activation, or gameplay
speed is not verified until owner QA. ## Bulk preparation of reviewed game profiles

`scripts/bottles-profiles-batch.sh` prepares the launch profiles for
**every explicitly selected game** in the same reviewed version-1 manifest.
It shares the safe native manager and exact registration checks of the
single-game workflow, but reads a `renderer` field for each game. The
discovery script initially records `"renderer": "auto"`: this is **not**
confirmation of an actual rendering backend. Review candidates, remove
unsupported/online/anti-cheat entries, and change ambiguous games to a
reviewed `"opengl"` or `"vulkan"` choice. No game is implicitly enrolled.

```fish
# Both are read-only regarding games; they export temporary preview profiles
# to validate all selected game configurations before any installation.
bash scripts/bottles-profiles-batch.sh plan "/absolute/reviewed-selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/source-policy.toml"

# One affirmative bulk preparation, only after closing all selected games.
bash scripts/bottles-profiles-batch.sh prepare "/absolute/reviewed-selection.json" "$HOME/.cargo/bin/intimatr-bottles" "/absolute/source-policy.toml" --offline-single-player --games-closed

bash scripts/bottles-profiles-batch.sh status "/absolute/reviewed-selection.json"
bash scripts/bottles-profiles-batch.sh remove "/absolute/reviewed-selection.json" --games-closed
```

The batch freezes the source manifest for the operation, checks exact game
registrations and profile rules, and writes only missing per-game managed
profiles. Every game's resolved Windows DLL-readable profile is previewed
in temporary storage first, including verification that speed is enabled.
Identical re-preparation is a no-op. Bulk removal preflights ownership for
every existing profile before removing any. If a late preparation fails,
only earlier fully prepared profiles from that invocation are eligible for
compensating removal; an incomplete current profile is preserved for
inspection.

This batch is **not globally atomic across environment, DLL, HUD, and
profiles** and does not yet share the base setup's recovery journal.
The new `bottles-full-setup.sh` entrypoint now orchestrates both steps
under one plan/apply, but their recovery mechanisms are still independent.
No game starts during preparation.

The optional `scripts/bottles-shortcut.sh` now creates an explicit
per-game XDG applications entry **only after a managed profile's read-only
launch preflight passes**. It writes to
`${XDG_DATA_HOME:-$HOME/.local/share}/applications/`, not
`~/.local/bin`, and never copies or installs an executable. The entry is
named using the game and bottle; its filename is based on a stable SHA-256
of the exact selected game directory.

```fish
# Read-only preview; requires a prepared profile and successful launch check.
bash scripts/bottles-shortcut.sh plan "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME"

# Optional explicit launcher installation. This does not start Wine.
bash scripts/bottles-shortcut.sh install "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME" --offline-single-player

# If speed timers should be enabled when the shortcut is clicked, explicitly
# add --experimental-four-timers to plan/install (it is saved in the entry).

bash scripts/bottles-shortcut.sh status "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME"
bash scripts/bottles-shortcut.sh remove "/absolute/bottles-root/BOTTLE/drive_c/Games/GAME"
```

The installer refuses an existing foreign or modified desktop entry and
treats a repeated identical installation as a no-op. Removal compares the
entire generated entry, including the optional timer opt-in, and refuses to
delete edited user files. Because Desktop Entry argument quoting has subtle
rules, unsupported path metacharacters are rejected rather than guessed.
This first implementation still requires a source checkout at the
same absolute path, and the app menu's actual visibility and launch behavior
remain for owner QA. No system-wide desktop registration or GUI is installed.

The combined project is not yet a fully verified, zero-configuration
launch-and-setup experience.

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
- The unified branch includes the native OpenGL `--mangohud-dlsym`
  path (originally developed in PR #6), a conservatively selected
  renderer-specific launch wrapper, per-game profile preparation, and
  opt-in XDG desktop shortcuts. It does **not** automatically refresh
  changed speed policies or verify desktop-launch behavior, HUD rendering,
  or gameplay speed.
  The current launch timing fields measure preflight and CLI overhead,
  not the game's visible or responsive startup time.
