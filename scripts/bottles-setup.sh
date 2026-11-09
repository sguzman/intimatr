#!/usr/bin/env bash
# Coordinate existing owned Bottles environment, Intimatr DLL and MangoHud
# installers for one reviewed game selection. Does not launch Wine/games.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-setup.sh plan MANIFEST ABSOLUTE_MANAGER [--all-bottles]
  bash scripts/bottles-setup.sh status MANIFEST ABSOLUTE_MANAGER [--all-bottles]
  bash scripts/bottles-setup.sh apply MANIFEST ABSOLUTE_MANAGER [--runtime ABSOLUTE_X64_INTIMATR_DLL] --offline-single-player --games-closed --bottles-closed [--all-bottles]

Use the reviewed version-1 games manifest from scripts/bottles-discover.sh.
--all-bottles applies *environment allowlist only* to all native Bottles.
DLL and HUD installation always remains limited to selected game directories.
EOF
  exit 2
}

[[ $# -ge 3 ]] || usage
action="$1"; manifest="$2"; manager="$3"; shift 3
[[ "$action" == plan || "$action" == status || "$action" == apply ]] || usage
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || usage
[[ "$manager" == /* && -f "$manager" && -x "$manager" && ! -L "$manager" ]] || usage
for tool in jq realpath sha256sum bottles-cli; do
  command -v "$tool" >/dev/null || { echo "Missing $tool" >&2; exit 2; }
done

all_bottles=false; offline=false; games_closed=false; bottles_closed=false; runtime=""
while (( $# )); do
  case "$1" in
    --all-bottles)
      [[ "$all_bottles" == false ]] || usage
      all_bottles=true; shift ;;
    --offline-single-player)
      [[ "$action" == apply && "$offline" == false ]] || usage
      offline=true; shift ;;
    --games-closed)
      [[ "$action" == apply && "$games_closed" == false ]] || usage
      games_closed=true; shift ;;
    --bottles-closed)
      [[ "$action" == apply && "$bottles_closed" == false ]] || usage
      bottles_closed=true; shift ;;
    --runtime)
      [[ "$action" == apply && -z "$runtime" && $# -ge 2 &&
         "$2" == /* && -f "$2" && ! -L "$2" ]] || usage
      runtime="$2"; shift 2 ;;
    *) usage ;;
  esac
done
if [[ "$action" == apply ]]; then
  [[ "$offline" == true && "$games_closed" == true && "$bottles_closed" == true ]] || usage
fi

# Freeze the reviewed selection once. Later Bottles/manager calls and
# installer stages must never reopen a manifest that another process could
# replace or modify between preflight and apply.
umask 077
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-bottles-setup.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
cat -- "$manifest" >"$tmp/reviewed-selection.json" ||
  { echo "Unable to snapshot selected-games manifest" >&2; exit 2; }
manifest="$tmp/reviewed-selection.json"

# Already reviewed by the owner; never select an unlisted game to patch.
jq -e '
  type == "object" and .version == 1 and
  (.games | type == "array" and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  all(.games[];
    type == "object" and
    (.game_dir | type == "string" and startswith("/") and
     (contains("\\") | not) and (contains("\t") | not) and
     (contains("\r") | not) and (contains("\n") | not)) and
    (.executable | type == "string" and
      test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$") and
      (contains("..") | not))
  )
' "$manifest" >/dev/null || { echo "Invalid selected-games manifest" >&2; exit 2; }

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
hud="$here/scripts/bottles-hud.sh"
proxy="$here/scripts/bottles-multi-game.sh"
[[ -f "$hud" && -f "$proxy" ]] || { echo "Missing Intimatr component installers" >&2; exit 2; }

root_raw="$(bottles-cli --json info bottles-path)"
[[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
  { echo "Native Bottles returned an invalid root path" >&2; exit 1; }
root="$(realpath -e -- "$root_raw")"
[[ -d "$root" ]] || { echo "Native Bottles root is not a directory" >&2; exit 1; }
inventory="$(bottles-cli --json list bottles)"
jq -e 'type == "object"' <<<"$inventory" >/dev/null ||
  { echo "Native Bottles inventory was not a name-keyed object" >&2; exit 1; }

: >"$tmp/new-dll.jsonl"; : >"$tmp/new-hud.jsonl"
declare -a bottle_names=() game_dirs=() game_exes=()
declare -A bottle_seen=()
while IFS=$'\t' read -r game exe; do
  [[ -d "$game" && ! -L "$game" && -f "$game/$exe" && ! -L "$game/$exe" ]] ||
    { echo "Missing or linked selected executable: $game/$exe" >&2; exit 1; }
  canonical="$(realpath -e -- "$game")"
  [[ "$canonical" == "$game" && "$canonical" == "$root/"* ]] ||
    { echo "Selected game does not resolve literally under native Bottles root: $game" >&2; exit 1; }
  relative="${canonical#"$root"/}"
  bottle="${relative%%/*}"
  tail_path="${relative#"$bottle"/}"
  [[ "$bottle" != "." && "$bottle" != ".." && "$bottle" != *"/"* &&
     "$tail_path" == drive_c/* ]] ||
    { echo "Selected game is not inside one native bottle C: drive: $game" >&2; exit 1; }
  jq -e --arg name "$bottle" 'has($name)' <<<"$inventory" >/dev/null ||
    { echo "Selected game bottle is absent from native Bottles: $bottle" >&2; exit 1; }
  if [[ -z "${bottle_seen[$bottle]+selected}" ]]; then
    bottle_seen[$bottle]=1; bottle_names+=("$bottle")
  fi
  game_dirs+=("$game"); game_exes+=("$exe")
done < <(jq -r '.games[] | [.game_dir, .executable] | @tsv' "$manifest")

if [[ "$all_bottles" == false && "${#bottle_names[@]}" -eq 0 ]]; then
  echo "No selected games. Use a reviewed nonempty manifest or --all-bottles for environment-only settings." >&2
  exit 2
fi

declare -a env_args=()
if [[ "$all_bottles" == true ]]; then
  env_args+=(--all)
else
  for bottle in "${bottle_names[@]}"; do env_args+=(--bottle "$bottle"); done
fi
echo "=== Native Bottles environment (${action}) ==="
if [[ "$action" == status ]]; then
  "$manager" bottle-env status "${env_args[@]}"
else
  "$manager" bottle-env plan "${env_args[@]}" --preset intimatr-hud
fi

# Snapshot every original/proxy state before making any changes.
for i in "${!game_dirs[@]}"; do
  game="${game_dirs[i]}"; exe="${game_exes[i]}"
  report="$("$manager" launch-proxy status --game-dir "$game")" ||
    { echo "Unmanaged or incomplete DLL deployment: $game" >&2; exit 1; }
  state="$(jq -er '.status | select(. == "installed" or . == "not_installed")' <<<"$report")" ||
    { echo "Unknown DLL status: $game" >&2; exit 1; }
  if [[ "$state" == installed ]]; then
    echo "DLL owned and verified: $game"
  else
    echo "DLL candidate: $game"
    jq -nc --arg game_dir "$game" --arg executable "$exe" \
      '{game_dir:$game_dir,executable:$executable}' >>"$tmp/new-dll.jsonl"
  fi
done
jq -s '{version:1,games:.}' "$tmp/new-dll.jsonl" >"$tmp/new-dll.json"
new_dll_count="$(jq '.games | length' "$tmp/new-dll.json")"
if (( new_dll_count )) && [[ "$action" != status ]]; then
  echo "=== Preflight $new_dll_count new DLL candidates ==="
  bash "$proxy" plan "$tmp/new-dll.json" "$manager"
fi

if (( ${#game_dirs[@]} )); then
  echo "=== Managed MangoHud configuration ==="
  hud_status="$(bash "$hud" status "$manifest")"
  printf '%s\n' "$hud_status"
  for i in "${!game_dirs[@]}"; do
    game="${game_dirs[i]}"; exe="${game_exes[i]}"
    if grep -Fxq -- "HUD absent: $game" <<<"$hud_status"; then
      jq -nc --arg game_dir "$game" --arg executable "$exe" \
        '{game_dir:$game_dir,executable:$executable}' >>"$tmp/new-hud.jsonl"
    elif ! grep -Fxq -- "HUD owned (unchanged): $game" <<<"$hud_status"; then
      echo "Refusing unmanaged or modified MangoHud configuration: $game" >&2
      exit 1
    fi
  done
fi
jq -s '{version:1,games:.}' "$tmp/new-hud.jsonl" >"$tmp/new-hud.json"
new_hud_count="$(jq '.games | length' "$tmp/new-hud.json")"
echo "Selection: ${#game_dirs[@]} games; $new_dll_count new DLLs; $new_hud_count new HUD configs."

if [[ "$action" != apply ]]; then
  echo "Read-only $action: no bottle, DLL or HUD files changed."
  exit 0
fi
if (( new_dll_count )); then
  if [[ -z "$runtime" ]]; then
    echo "A new DLL install requires --runtime ABSOLUTE_X64_INTIMATR_DLL; refusing all changes." >&2
    exit 2
  fi
  # Preflight ALL runtime/build prerequisites before editing bottle.yml.
  # Previously a missing MinGW compiler or invalid DLL was discovered only
  # inside the proxy installer, after the environment stage had committed.
  for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-dlltool; do
    command -v "$tool" >/dev/null ||
      { echo "Missing DLL build prerequisite before applying: $tool" >&2; exit 2; }
  done
  [[ -f "$here/scripts/generate-steam-api64-forwarders.sh" &&
     -f "$here/native/launch_proxy/steam_api64_bridge.c" &&
     -f "$here/config/bottles-proxy-template.toml" ]] ||
    { echo "Incomplete Intimatr DLL forwarder source checkout; refusing all changes." >&2; exit 2; }
  if ! x86_64-w64-mingw32-objdump -f "$runtime" 2>/dev/null |
      grep -Eq 'file format pei-x86-64'; then
    echo "Runtime is not a readable x64 Windows PE DLL: $runtime; refusing all changes." >&2
    exit 2
  fi
fi

# Lock the canonical native Bottles directory itself, avoiding creation of
# a lockfile that could collide with or follow an unmanaged user symlink.
# The recovery command holds this same advisory lock during rollback.
command -v flock >/dev/null ||
  { echo "Missing flock for exclusive Bottles setup" >&2; exit 2; }
exec 9<"$root"
flock -n 9 ||
  { echo "Another Intimatr setup/recovery holds the native Bottles lock" >&2; exit 1; }

# Persist a per-attempt recovery record before modifying any Bottles files.
# It contains only reviewed selections and the original ownership inventory,
# never credentials, game binaries, or a second copy of the DLL runtime.
# The directory is inside the canonical native Bottles root and has mode 0700.
journal="$(mktemp -d "$root/.intimatr-setup.XXXXXXXX")" ||
  { echo "Cannot create durable Bottles setup journal" >&2; exit 1; }
cp -- "$manifest" "$journal/selection.json"
cp -- "$tmp/new-dll.json" "$journal/new-dll.json"
cp -- "$tmp/new-hud.json" "$journal/new-hud.json"
"$manager" bottle-env status "${env_args[@]}" --preset intimatr-hud >"$journal/env-before.jsonl"
jq -s -e 'all(.[]; .operation == "status" and
  (.bottle | type == "string") and
  (.ownership | type == "string") and
  (.allowlist_entries_to_add | type == "array"))'   "$journal/env-before.jsonl" >/dev/null ||
  { echo "Native manager returned invalid environment recovery inventory; refusing all edits. Journal: $journal" >&2; exit 1; }
jq -n --arg root "$root" --argjson all_bottles "$all_bottles"   '{version:1,root:$root,all_bottles:$all_bottles}' >"$journal/metadata.json"
# A stage marker is committed by rename so a killed process leaves its last
# completed boundary visible. A stage marked "started" can be incomplete.
mark_stage() {
  printf '%s\n' "$1" >"$journal/.stage-next"
  mv -- "$journal/.stage-next" "$journal/stage"
}
mark_stage prepared
echo "Recovery journal: $journal"

# No global atomicity is promised: each component has its own guard/backup.
# An interrupted run preserves this journal for explicit integrity-checked
# recovery; never assume a stage succeeded merely because it was started.
stage=environment
trap 'rc=$?; if (( rc )); then echo "INCOMPLETE at $stage. Journal: $journal. Inspect ownership before retrying; no destructive automatic rollback." >&2; fi; rm -rf -- "$tmp"' EXIT
mark_stage environment-started
echo "=== Applying inherited variable allowlists ==="
"$manager" bottle-env apply "${env_args[@]}" --preset intimatr-hud --bottles-closed
mark_stage environment-complete
stage=DLL
if (( new_dll_count )); then
  mark_stage dll-started
  echo "=== Installing selected DLL proxies ==="
  bash "$proxy" install "$tmp/new-dll.json" "$manager" "$runtime" \
    --offline-single-player --games-closed
  mark_stage dll-complete
fi
stage=MangoHud
if (( new_hud_count )); then
  mark_stage hud-started
  echo "=== Installing selected MangoHud configs ==="
  bash "$hud" install "$tmp/new-hud.json" --offline-single-player --games-closed
  mark_stage hud-complete
fi
stage=complete
mark_stage complete
echo "Selected Bottles configuration applied. Live renderer output and speed require in-game QA."
