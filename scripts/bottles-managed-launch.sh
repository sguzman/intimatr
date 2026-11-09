#!/usr/bin/env bash
# Owned per-game launch profile. No global app settings or binaries are copied.
# A prepared game can later be checked/launched using only its game directory.
set -euo pipefail
usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-managed-launch.sh plan MANIFEST MANAGER GAME_DIR SOURCE_TOML --renderer (auto|opengl|vulkan)
  bash scripts/bottles-managed-launch.sh prepare MANIFEST MANAGER GAME_DIR SOURCE_TOML --renderer (auto|opengl|vulkan) --offline-single-player --games-closed
  bash scripts/bottles-managed-launch.sh status GAME_DIR
  bash scripts/bottles-managed-launch.sh check GAME_DIR --offline-single-player [--experimental-four-timers]
  bash scripts/bottles-managed-launch.sh launch GAME_DIR --offline-single-player [--experimental-four-timers]
  bash scripts/bottles-managed-launch.sh remove GAME_DIR --games-closed
Plan and status are read-only; prepare writes only a new private
.intimatr-managed-launch directory beside the selected game executable.
EOF
  exit 2
}
(( $# >= 2 )) || usage
action="$1"; shift
case "$action" in plan|prepare|status|check|launch|remove) ;; *) usage ;; esac
for tool in jq sha256sum cmp realpath bottles-cli; do
  command -v "$tool" >/dev/null || { echo "Missing required command: $tool" >&2; exit 2; }
done
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
if [[ "$action" == plan || "$action" == prepare ]]; then
  (( $# >= 4 )) || usage
  selection_file="$1"; manager="$2"; game="$3"; source="$4"; shift 4
  [[ "$selection_file" == /* && -f "$selection_file" && ! -L "$selection_file" &&
     "$manager" == /* && -x "$manager" && -f "$manager" && ! -L "$manager" &&
     "$source" == /* && -f "$source" && ! -L "$source" ]] || usage
  renderer=""; offline=false; games_closed=false
  while (( $# )); do
    case "$1" in
      --renderer)
        [[ -z "$renderer" && $# -ge 2 ]] || usage
        renderer="$2"; shift 2 ;;
      --offline-single-player)
        [[ "$action" == prepare && "$offline" == false ]] || usage
        offline=true; shift ;;
      --games-closed)
        [[ "$action" == prepare && "$games_closed" == false ]] || usage
        games_closed=true; shift ;;
      *) usage ;;
    esac
  done
  [[ "$renderer" == auto || "$renderer" == opengl || "$renderer" == vulkan ]] || usage
  if [[ "$action" == prepare ]]; then
    [[ "$offline" == true && "$games_closed" == true ]] || usage
  fi
else
  game="$1"; shift
  [[ "$game" == /* ]] || usage
  timers=false; offline=false; games_closed=false
  while (( $# )); do
    case "$1" in
      --offline-single-player)
        [[ ( "$action" == check || "$action" == launch ) && "$offline" == false ]] || usage
        offline=true; shift ;;
      --experimental-four-timers)
        [[ ( "$action" == check || "$action" == launch ) && "$timers" == false ]] || usage
        timers=true; shift ;;
      --games-closed)
        [[ "$action" == remove && "$games_closed" == false ]] || usage
        games_closed=true; shift ;;
      *) usage ;;
    esac
  done
  if [[ "$action" == check || "$action" == launch ]]; then
    [[ "$offline" == true ]] || usage
  elif [[ "$action" == remove ]]; then
    [[ "$games_closed" == true ]] || usage
  fi
fi
[[ -d "$game" && ! -L "$game" && "$(realpath -e -- "$game")" == "$game" ]] ||
  { echo "Selected game must be a canonical existing directory" >&2; exit 1; }
managed="$game/.intimatr-managed-launch"
checksum_file="$managed/snapshots.sha256"

sha_file() { sha256sum < "$1" | cut -d' ' -f1; }
check_owned() {
  [[ -d "$managed" && ! -L "$managed" ]] ||
    { echo "No complete managed launch directory: $managed" >&2; return 1; }
  for file in launch.json speed.toml snapshots.sha256; do
    [[ -f "$managed/$file" && ! -L "$managed/$file" ]] ||
      { echo "Incomplete or symlinked managed launch file: $file" >&2; return 1; }
  done
  (
    cd -- "$managed"
    sha256sum -- launch.json speed.toml | cmp -s - snapshots.sha256
  ) || { echo "Managed launch checksum mismatch; refusing action" >&2; return 1; }
  jq -e --arg game "$game" --arg profile "$managed/speed.toml" '
    type == "object" and .version == 1 and .game_dir == $game and
    .profile == $profile and
    (.manager | type == "string" and startswith("/")) and
    (.source | type == "string" and startswith("/")) and
    (.source_sha256 | type == "string" and test("^[a-f0-9]{64}$")) and
    (.renderer == "auto" or .renderer == "opengl" or .renderer == "vulkan") and
    (.selection | type == "object") and .selection.game_dir == $game
  ' "$managed/launch.json" >/dev/null ||
    { echo "Managed launch metadata is not bound to this game" >&2; return 1; }
}
if [[ "$action" == plan || "$action" == prepare ]]; then
  root_raw="$(bottles-cli --json info bottles-path)"
  [[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
    { echo "Invalid native Bottles directory" >&2; exit 1; }
  root="$(realpath -e -- "$root_raw")"
  [[ "$game" == "$root/"* ]] ||
    { echo "Selected game is outside native Bottles root" >&2; exit 1; }
  selection="$(jq -ce --arg dir "$game" '
    if type != "object" or .version != 1 or (.games | type) != "array" then error("Invalid reviewed manifest")
    else [.games[] | select(.game_dir == $dir)] as $matches
    | if ($matches | length) != 1 then error("Need exactly one reviewed game") else $matches[0] end
    end
  ' "$selection_file")" || { echo "Missing or ambiguous reviewed selection" >&2; exit 1; }
  bottle="$(jq -er '.bottle | select(type == "string" and length > 0)' <<<"$selection")"
  exe="$(jq -er '.executable | select(type == "string" and test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$"))' <<<"$selection")"
  program="$(jq -er '.registered_program | select(type == "string" and length > 0)' <<<"$selection")"
  windows="$(jq -er '.windows_executable | select(type == "string" and length > 0)' <<<"$selection")"
  relative="${game#"$root"/}"
  name="${relative%%/*}"
  c_path="${relative#"$name"/}"
  [[ "$bottle" == "$name" && "$c_path" == drive_c/* &&
     -f "$game/$exe" && ! -L "$game/$exe" ]] ||
    { echo "Reviewed game is not in its selected native bottle C: drive" >&2; exit 1; }
  from_c="${c_path#drive_c/}/$exe"
  slash_path="${windows//\\//}"
  [[ "$slash_path" =~ ^[cC]:/ && "${slash_path:3}" == "$from_c" ]] ||
    { echo "Reviewed Windows registration is not the selected executable" >&2; exit 1; }
  registered="$(bottles-cli --json programs -b "$bottle")"
  jq -e --arg name "$program" --arg path "$windows" '
    type == "array" and
    ([.[] | select(.name == $name)] | length == 1) and
    ([.[] | select(.name == $name and .path == $path)] | length == 1)
  ' <<<"$registered" >/dev/null ||
    { echo "Native Bottles registration changed or is ambiguous" >&2; exit 1; }
  [[ ! -e "$managed" && ! -L "$managed" ]] ||
    { echo "Managed launch already exists (or incomplete): $managed" >&2; exit 1; }
  # The existing Rust resolver is read-only. The later export will validate
  # and deliver a DLL-readable profile, but no launch is attempted here.
  "$manager" profile-check --config "$source" --bottle "$bottle" --exe "$windows" >/dev/null
  if [[ "$action" == plan ]]; then
    jq -n --arg game "$game" --arg bottle "$bottle" --arg program "$program" \
      --arg renderer "$renderer" \
      '{operation:"managed_launch_plan",selected_game:$game,
        bottle:$bottle,registered_program:$program,renderer:$renderer,
        writes:[".intimatr-managed-launch/speed.toml",
                ".intimatr-managed-launch/launch.json",
                ".intimatr-managed-launch/snapshots.sha256"],
        game_started:false,modified:false}'
    exit 0
  fi
  umask 077
  # Exclusive mkdir refuses existing user-owned state and symlink races.
  mkdir -- "$managed" || { echo "Cannot create new private managed launch directory" >&2; exit 1; }
  profile="$managed/speed.toml"
  export_report="$("$manager" profile-export --config "$source" --bottle "$bottle" \
    --exe "$windows" --output "$profile")" ||
    { echo "Profile export failed. Incomplete directory retained: $managed" >&2; exit 1; }
  jq -e --arg profile "$profile" '
    .operation == "explicit_launch_profile_export" and
    .output_linux_file == $profile and .resolved_speed_enabled == true
  ' <<<"$export_report" >/dev/null ||
    { echo "Profile export is disabled or invalid; incomplete directory retained: $managed" >&2; exit 1; }
  [[ -f "$profile" && ! -L "$profile" ]] ||
    { echo "Exported profile is missing or linked" >&2; exit 1; }
  source_hash="$(sha_file "$source")"
  jq -nc --arg game_dir "$game" --arg manager "$manager" --arg source "$source" \
    --arg source_sha256 "$source_hash" --arg profile "$profile" \
    --arg renderer "$renderer" --argjson selection "$selection" \
    '{version:1,game_dir:$game_dir,manager:$manager,source:$source,
      source_sha256:$source_sha256,profile:$profile,renderer:$renderer,
      selection:$selection}' >"$managed/launch.json"
  (
    cd -- "$managed"
    sha256sum -- launch.json speed.toml >snapshots.sha256
  )
  check_owned
  echo "Managed game profile prepared: $game"
  echo "Use: bash scripts/bottles-managed-launch.sh check \"$game\" --offline-single-player"
  exit 0
fi

check_owned || exit 1
if [[ "$action" == status ]]; then
  jq -n --arg game "$game" \
    '{operation:"managed_launch_status",game_dir:$game,integrity:"owned_unchanged",
      game_started:false,gameplay_verified:false}'
  exit 0
fi
# No pending writes are permitted from check/launch. Refuse a source policy
# revision until the selected delivered profile is deliberately rebuilt.
source="$(jq -er .source "$managed/launch.json")"
manager="$(jq -er .manager "$managed/launch.json")"
renderer="$(jq -er .renderer "$managed/launch.json")"
source_hash="$(jq -er .source_sha256 "$managed/launch.json")"
[[ "$source" == /* && -f "$source" && ! -L "$source" &&
   "$manager" == /* && -x "$manager" && -f "$manager" && ! -L "$manager" ]] ||
  { echo "Managed launch source or native manager is missing" >&2; exit 1; }
[[ "$(sha_file "$source")" == "$source_hash" ]] ||
  { echo "Source speed policy changed; refusing stale delivered profile" >&2; exit 1; }

if [[ "$action" == remove ]]; then
  count="$(find "$managed" -mindepth 1 -maxdepth 1 -printf '. ' | wc -w)"
  [[ "$count" -eq 3 ]] ||
    { echo "Extra files in managed directory; refusing removal" >&2; exit 1; }
  rm -- "$managed/launch.json" "$managed/speed.toml" "$checksum_file"
  rmdir -- "$managed"
  echo "Removed only the owned per-game launch profile: $game"
  exit 0
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-managed-launch.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
jq -n --slurpfile config "$managed/launch.json" \
  '{version:1,games:[$config[0].selection]}' >"$tmp/selection.json"
args=("$action" "$tmp/selection.json" "$manager" "$game" "$source"
  "$managed/speed.toml" --renderer "$renderer" --offline-single-player)
[[ "$timers" == false ]] || args+=(--experimental-four-timers)
bash "$here/scripts/bottles-launch.sh" "${args[@]}"
