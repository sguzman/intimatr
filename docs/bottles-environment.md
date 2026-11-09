# Managed Bottles inherited environment (initial CLI stage)

The native \`intimatr-bottles bottle-env\` subcommand makes **bulk inherited
environment allowlist** changes for explicitly selected native Bottles
prefixes. It does not change environment assignment values, install game DLLs,
install MangoHud, create game shortcuts, or launch Wine. Those separate
features are tracked in issue #4. This first stage eliminates repeated GUI
allowlist edits while preserving the rest of each bottle's configuration.

Use the installed Rust CLI (installed by \`cargo install --path .\`, not by
copying binaries into a custom directory).

\`\`\`fish
# Read-only: discover requested additions in selected bottles.
intimatr-bottles bottle-env plan --all --preset intimatr-hud
# Or choose only named bottles: --bottle Eastward --bottle "Second Game"

# With selected games AND Bottles itself closed, edit only those allowlists.
intimatr-bottles bottle-env apply --all --preset intimatr-hud --bottles-closed

# Read-only: inspect managed/unmanaged status.
intimatr-bottles bottle-env status --bottle Eastward --preset intimatr-hud

# With Bottles closed, restore original bottle.yml only if unchanged since apply.
intimatr-bottles bottle-env restore --bottle Eastward --bottles-closed
\`\`\`

Use repeated \`--allow NAME\` instead of the \`--preset intimatr-hud\` bundle
for arbitrary uppercase environment names. The bundled preset permits
\`INTIMATR_SPEED_PROFILE\`, \`INTIMATR_EXPERIMENTAL_STARTUP_TIMERS\`,
\`MANGOHUD\`, \`INTIMATR_HUD_TELEMETRY\`, \`MANGOHUD_CONFIGFILE\`, and
\`LD_PRELOAD\` *to be inherited* by a restricted bottle. It does not set their
values or cause MangoHud to render automatically.

### Integrity and limitations

- Explicit bottle names or `--all` only. No unrequested mass operation; review the plan before using `--all` in apply.
- Preflights **all** selected bottles before any edit, refusing stale native
  inventory, unsupported YAML layouts, overrides, symlinks and duplicate names.
- Edits the ordinary top-level PyYAML block list while preserving every
  unrelated configuration byte. No lossy YAML round-trip. Unrestricted
  environment inheritance requires no change.
- Per-bottle backup and SHA-256 ownership marker are created with restrictive
  permissions. Updates are staged and renamed atomically. Removal refuses
  modifications by Bottles or another tool since the Intimatr update.
- Multiple-bottle apply/restore is **not globally atomic**. On partial failure,
  inspect the per-bottle status and restore eligible owned entries explicitly.
- Bottle GUI and processes must be closed for writes. The
  \`--bottles-closed\` flag is an acknowledgment, not a process inspection.
- This feature does **not** automatically deploy Intimatr's DLL. Continue to
  use the separately reviewed \`bottles-multi-game.sh\` installer for explicitly
  selected compatible *offline* games. The combined manager is unfinished.

Any installed DLL/HUD status remains separate from runtime-confirmed
renderer output or observed game-time acceleration.
