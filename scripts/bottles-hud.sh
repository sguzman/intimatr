#!/usr/bin/env bash
# Generate an Intimatr-branded MangoHud overlay for explicitly selected Bottles
# games. No process injection, graphics hooks, or global MangoHud config edits.
set -euo pipefail

usage() {
  echo "Usage: bash scripts/bottles-hud.sh (plan MANIFEST_JSON | status MANIFEST_JSON | diagnose MANIFEST_JSON | install MANIFEST_JSON --offline-single-player --games-closed | remove MANIFEST_JSON --offline-single-player --games-closed)" >&2
  exit 2
}

[[ $# -ge 2 ]] || usage
action="$1"
manifest="$2"
shift 2
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || usage
case "$action" in
  plan|status|diagnose) [[ $# == 0 ]] || usage ;;
  install|remove)
    [[ $# == 2 && "$1" == --offline-single-player && "$2" == --games-closed ]] || usage
    ;;
  *) usage ;;
esac
command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 2; }
jq -e '
  type == "object" and .version == 1 and
  (.games | type == "array" and length >= 1 and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  all(.games[];
    type == "object" and
    (.game_dir | type == "string" and startswith("/") and
       (test("[\\t\\r\\n]") | not)) and
    (.executable | type == "string" and
       test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$") and
       (contains("..") | not))
  )' "$manifest" >/dev/null || {
    echo "Invalid, duplicate, or overlarge selection manifest" >&2; exit 2;
  }
declare -a games=() exes=()
while IFS=$'\t' read -r game exe; do
  games+=("$game")
  exes+=("$exe")
done < <(jq -r '.games[] | [.game_dir, .executable] | @tsv' "$manifest")

marker="intimatr-mangohud.sha256"
config="MangoHud.conf"
rate="intimatr-hud-rate.txt"

# MangoHud exec runs under /bin/sh. Escape the complete path as a single
# POSIX shell argument. Dollar expressions and quotes stay literal filename
# characters; tab/newline are rejected by the manifest schema.
quote_posix_argument() {
  local value="$1"
  value="${value//\'/\'\\\'\'}"
  printf "'%s'" "$value"
}

# Fully validate every item before modifying one. Never overwrite user
# settings, or an already-installed config from another tool.
for i in "${!games[@]}"; do
  game="${games[i]}"
  exe="${exes[i]}"
  [[ -d "$game" && ! -L "$game" && -f "$game/$exe" && ! -L "$game/$exe" ]] ||
    { echo "Invalid game/executable: $game/$exe" >&2; exit 1; }
  # HUD removal remains possible after the separate DLL proxy is uninstalled.
  if [[ "$action" == plan || "$action" == install ]]; then
    [[ -f "$game/intimatr-launch-deployment.json" && -f "$game/intimatr.dll" ]] ||
      { echo "Intimatr proxy not found in: $game" >&2; exit 1; }
  fi
  if [[ "$action" == install ]]; then
    [[ ! -e "$game/$config" && ! -L "$game/$config" && ! -e "$game/$marker" && ! -L "$game/$marker" ]] ||
      { echo "Refusing existing MangoHud configuration or marker: $game" >&2; exit 1; }
  elif [[ "$action" == remove ]]; then
    [[ -f "$game/$config" && ! -L "$game/$config" && -f "$game/$marker" && ! -L "$game/$marker" ]] ||
      { echo "No safely managed MangoHud config found: $game" >&2; exit 1; }
    expected="$(cat "$game/$marker")"
    observed="$(sha256sum "$game/$config" | cut -d' ' -f1)"
    [[ "$expected" == "$observed" ]] ||
      { echo "MangoHud config was modified; refusing removal: $game" >&2; exit 1; }
  fi
done

# Diagnostics report *host-side* configuration and a bounded speed sample
# only. No renderer, process, timer-hook, or gameplay assertion is possible.
if [[ "$action" == diagnose ]]; then
  now="$(date +%s)"
  mangohud_present=false
  if command -v mangohud >/dev/null; then mangohud_present=true; fi
  declare -a records=()
  for i in "${!games[@]}"; do
    game="${games[i]}"
    executable="${exes[i]}"
    config_state="absent"
    if [[ -e "$game/$config" || -L "$game/$config" ||
          -e "$game/$marker" || -L "$game/$marker" ]]; then
      config_state="unmanaged_or_incomplete"
      if [[ -f "$game/$config" && ! -L "$game/$config" &&
            -f "$game/$marker" && ! -L "$game/$marker" ]]; then
        expected="$(cat -- "$game/$marker" 2>/dev/null)" || expected=""
        observed="$(sha256sum -- "$game/$config" 2>/dev/null | cut -d' ' -f1)" || observed=""
        config_state="modified"
        if [[ "$expected" =~ ^[0-9a-fA-F]{64}$ &&
              "$expected" == "$observed" ]]; then
          config_state="owned_unchanged"
        fi
      fi
    fi

    sample="$game/$rate"
    speed_state="missing"
    speed_value=""
    age_seconds=""
    if [[ -e "$sample" || -L "$sample" ]]; then
      speed_state="unsafe_or_invalid"
      if [[ -f "$sample" && ! -L "$sample" ]]; then
        sample_size="$(stat -c %s -- "$sample" 2>/dev/null)" || sample_size=""
        sample_mtime="$(stat -c %Y -- "$sample" 2>/dev/null)" || sample_mtime=""
        if [[ "$sample_size" =~ ^[0-9]+$ && "$sample_mtime" =~ ^[0-9]+$ &&
              "$sample_size" -ge 1 && "$sample_size" -le 16 ]]; then
          candidate="$(cat -- "$sample" 2>/dev/null)" || candidate=""
          if [[ "$candidate" =~ ^([0-9]+[.][0-9]{2}x|--)$ ]]; then
            age_seconds="$((now - sample_mtime))"
            if (( age_seconds < 0 )); then
              speed_state="future_timestamp"
            elif (( age_seconds > 6 )); then
              speed_state="expired"
            else
              speed_state="fresh"
              speed_value="$candidate"
            fi
          fi
        fi
      fi
    fi
    records+=("$(jq -nc --arg directory "$game" --arg executable "$executable" \
      --arg config_state "$config_state" --arg speed_state "$speed_state" \
      --arg speed_value "$speed_value" --arg age "$age_seconds" \
      '{game_dir:$directory, executable:$executable,
        managed_config_state:$config_state,
        speed_sample_state:$speed_state,
        speed_sample_age_seconds:(if $age=="" then null else ($age|tonumber) end),
        observed_installed_clock_rate:(if $speed_value=="" then null else $speed_value end),
        in_game_rendering_verified:false, gameplay_speed_verified:false}')")
  done
  printf '%s\n' "${records[@]}" | jq -s --argjson now "$now" \
    --argjson mangohud "$mangohud_present" \
    '{operation:"read_only_bottles_hud_diagnostics",
      inspected_at_unix_seconds:$now,
      mangohud_command_present_on_host:$mangohud,
      in_game_rendering_verified:false,
      gameplay_speed_verified:false,
      files_modified:false,
      games:.}'
  exit 0
fi

if [[ "$action" == status ]]; then
  for game in "${games[@]}"; do
    if [[ ! -e "$game/$config" && ! -L "$game/$config" &&
          ! -e "$game/$marker" && ! -L "$game/$marker" ]]; then
      echo "HUD absent: $game"
    elif [[ -f "$game/$config" && ! -L "$game/$config" &&
            -f "$game/$marker" && ! -L "$game/$marker" ]]; then
      expected="$(cat "$game/$marker")"
      observed="$(sha256sum "$game/$config" | cut -d' ' -f1)"
      if [[ "$expected" == "$observed" ]]; then
        echo "HUD owned (unchanged): $game"
      else
        echo "HUD modified (refusing automatic removal): $game"
      fi
    else
      echo "HUD unmanaged or incomplete: $game"
    fi
  done
  echo "Status is read-only; no games or HUD configs were modified."
  exit 0
fi
if [[ "$action" == plan ]]; then
  for i in "${!games[@]}"; do
    echo "HUD candidate: ${exes[i]} in ${games[i]}"
  done
  echo "All checks read-only; MangoHud must be enabled separately for each bottle."
  exit 0
fi
if [[ "$action" == remove ]]; then
  for game in "${games[@]}"; do
    rm -- "$game/$config"
    rm -- "$game/$marker"
    echo "Removed owned MangoHud configuration in: $game"
  done
  exit 0
fi

# A batch installation must not leave a successful first game configured
# when a later game fails. Roll back only files created in this invocation,
# and only while their recorded checksums still match. Preserve modified
# files rather than deleting another process's work.
umask 077
declare -a installed_this_run=()
declare -a installed_hashes=()
pending_config=""
pending_checksum=""
rollback_install() {
  local result=$?
  if (( result != 0 )); then
    echo "HUD batch install failed; restoring unchanged files created in this attempt" >&2
    if [[ -n "$pending_config" && -f "$pending_config" && ! -L "$pending_config" ]]; then
      local actual
      actual="$(sha256sum -- "$pending_config" | cut -d' ' -f1)" || actual=""
      if [[ "$actual" == "$pending_checksum" ]]; then
        rm -f -- "$pending_config" || true
      else
        echo "Preserving modified or unverifiable partial HUD config: $pending_config" >&2
      fi
    fi
    local game expected observed wanted i
    for i in "${!installed_this_run[@]}"; do
      game="${installed_this_run[i]}"
      wanted="${installed_hashes[i]}"
      if [[ -f "$game/$config" && ! -L "$game/$config" &&
            -f "$game/$marker" && ! -L "$game/$marker" ]]; then
        expected="$(cat -- "$game/$marker")" || expected=""
        observed="$(sha256sum -- "$game/$config" | cut -d' ' -f1)" || observed=""
        if [[ -n "$wanted" && "$wanted" == "$expected" && "$wanted" == "$observed" ]]; then
          rm -f -- "$game/$config" "$game/$marker" || true
          echo "Rolled back unchanged HUD config: $game" >&2
        else
          echo "Preserving modified HUD config after failure: $game" >&2
        fi
      else
        echo "Preserving incomplete or modified HUD files: $game" >&2
      fi
    done
  fi
}
trap rollback_install EXIT
for game in "${games[@]}"; do
  # Quote the game-specific rate path without evaluating filename text.
  quoted_rate_path="$(quote_posix_argument "$game/$rate")"
  output="$game/$config"
  (
    set -C
    cat >"$output" <<EOF
# Intimatr performance HUD; managed by scripts/bottles-hud.sh
# MangoHud supplies FPS, CPU/GPU use, and memory. Intimatr supplies speed.
legacy_layout=0
position=top-left
background_alpha=0.55
font_size=20
fps
frametime
cpu_stats
gpu_stats
ram
vram
procmem
custom_text_center=INTIMATR
custom_text=Speed
# Telemetry writes at least every 2s. Reject samples older than 6s.
# Command substitutions are escaped so they run inside MangoHud, not here.
exec=mtime=\$(stat -c %Y ${quoted_rate_path} 2>/dev/null) && now=\$(date +%s) && [ \$((now - mtime)) -ge 0 ] && [ \$((now - mtime)) -le 6 ] && [ -s ${quoted_rate_path} ] && cat ${quoted_rate_path} 2>/dev/null || printf '%s\n' '--'
toggle_hud=Shift_R+F12
EOF
  ) || { echo "Cannot create MangoHud config in: $game" >&2; exit 1; }
  checksum="$(sha256sum "$output" | cut -d' ' -f1)"
  pending_config="$output"
  pending_checksum="$checksum"
  printf '%s\n' "$checksum" >"$game/$marker" || {
    echo "Failed to write ownership marker" >&2
    exit 1
  }
  installed_this_run+=("$game")
  installed_hashes+=("$checksum")
  pending_config=""
  pending_checksum=""
  echo "Configured Intimatr HUD for: $game"
done
trap - EXIT
echo "Enable MangoHud in Bottles for the selected games. With startup timing opted in, the DLL writes observed speed to the game-specific rate file."
