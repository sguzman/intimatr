#!/usr/bin/env bash
# Generate an Intimatr-branded MangoHud overlay for explicitly selected Bottles
# games. No process injection, graphics hooks, or global MangoHud config edits.
set -euo pipefail

usage() {
  echo "Usage: bash scripts/bottles-hud.sh (plan MANIFEST_JSON | status MANIFEST_JSON | install MANIFEST_JSON --offline-single-player --games-closed | remove MANIFEST_JSON --offline-single-player --games-closed)" >&2
  exit 2
}

[[ $# -ge 2 ]] || usage
action="$1"
manifest="$2"
shift 2
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || usage
case "$action" in
  plan|status) [[ $# == 0 ]] || usage ;;
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
       (test("[\\t\\r\\n\\x22\\x27]") | not)) and
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

umask 077
for game in "${games[@]}"; do
  # Quote paths as an argument to MangoHud's exec shell. Manifest validation
  # rejects both quote types, controls, and embedded newlines.
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
# Telemetry refreshes every 2s while active; abandon values older than 6s.
# Escape '
toggle_hud=Shift_R+F12
EOF
  ) || { echo "Cannot create MangoHud config in: $game" >&2; exit 1; }
  checksum="$(sha256sum "$output" | cut -d' ' -f1)"
  printf '%s\n' "$checksum" >"$game/$marker" || {
    rm -f -- "$output"
    echo "Failed to write ownership marker" >&2
    exit 1
  }
  echo "Configured Intimatr HUD for: $game"
done
echo "Enable MangoHud in Bottles for the selected games. With startup timing opted in, the DLL writes observed speed to the game-specific rate file."
 here so the shell command runs in MangoHud, not at install time.
exec=mtime=\$(stat -c %Y '$game/$rate' 2>/dev/null) && now=\$(date +%s) && [ \$((now - mtime)) -ge 0 ] && [ \$((now - mtime)) -le 6 ] && [ -s '$game/$rate' ] && cat '$game/$rate' 2>/dev/null || printf '%s\n' '--'
toggle_hud=Shift_R+F12
EOF
  ) || { echo "Cannot create MangoHud config in: $game" >&2; exit 1; }
  checksum="$(sha256sum "$output" | cut -d' ' -f1)"
  printf '%s\n' "$checksum" >"$game/$marker" || {
    rm -f -- "$output"
    echo "Failed to write ownership marker" >&2
    exit 1
  }
  echo "Configured Intimatr HUD for: $game"
done
echo "Enable MangoHud in Bottles for the selected games. With startup timing opted in, the DLL writes observed speed to the game-specific rate file."
