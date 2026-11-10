#!/usr/bin/env bash
# Read-only, per-game launch preflight across an explicitly reviewed selection.
# Does NOT start Wine, load a DLL, arm timing hooks, or claim visible HUD.
set -euo pipefail
usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-qa-preflight.sh ABSOLUTE_REVIEWED_MANIFEST --offline-single-player [--experimental-four-timers]
The manifest must identify 1..32 distinct, registered offline Bottles games
with game_dir, bottle, executable, registered_program, windows_executable,
and renderer (opengl|vulkan|auto) for every selection.
Reports one JSON record per selected game plus one aggregate JSON record.
No game is launched; readiness does not establish in-game HUD or speed.
EOF
  exit 2
}
(( $# >= 2 )) || usage
manifest="$1"; shift
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || usage
offline=false; timers=false
while (( $# )); do
  case "$1" in
    --offline-single-player)
      [[ "$offline" == false ]] || usage
      offline=true; shift ;;
    --experimental-four-timers)
      [[ "$timers" == false ]] || usage
      timers=true; shift ;;
    *) usage ;;
  esac
done
[[ "$offline" == true ]] || usage
for cmd in jq sha256sum realpath; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 2; }
done
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
single="$here/scripts/bottles-managed-launch.sh"
[[ -f "$single" && ! -L "$single" ]] ||
  { echo "Missing Intimatr managed launcher" >&2; exit 2; }
umask 077
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-qa-preflight.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
cp -- "$manifest" "$tmp/reviewed.json"
manifest="$tmp/reviewed.json"
jq -e '
  type == "object" and .version == 1 and
  (.games | type == "array" and length > 0 and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  all(.games[];
    type == "object" and
    (.game_dir | type == "string" and startswith("/") and
      (test("[\t\r\n]") | not) and (contains("\\") | not)) and
    (.bottle | type == "string" and length > 0) and
    (.executable | type == "string" and
      test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$")) and
    (.registered_program | type == "string" and length > 0) and
    (.windows_executable | type == "string" and length > 0) and
    (.renderer == "auto" or .renderer == "opengl" or .renderer == "vulkan")
  )
' "$manifest" >/dev/null ||
  { echo "Invalid, empty or ambiguous reviewed QA selection" >&2; exit 2; }

total=0; ready=0; blocked=0
while IFS= read -r selection; do
  total=$((total+1))
  game="$(jq -er '.game_dir' <<<"$selection")"
  bottle="$(jq -er '.bottle' <<<"$selection")"
  renderer="$(jq -er '.renderer' <<<"$selection")"
  reason=""
  managed="$game/.intimatr-managed-launch"
  # The exact reviewed selection must match the one used at preparation.
  # Neither a checksum-valid profile for a different game nor a changed
  # renderer can silently pass the pre-live-QA checkpoint.
  if ! bash "$single" status "$game" >"$tmp/owned-status.json" 2>"$tmp/error"; then
    reason=managed_profile_not_intact
  elif ! jq -e --argjson reviewed "$selection" '
    .selection == $reviewed and .renderer == $reviewed.renderer
  ' "$managed/launch.json" >/dev/null 2>"$tmp/error"; then
    reason=reviewed_selection_differs_from_prepared_profile
  else
    args=(check "$game" --offline-single-player)
    [[ "$timers" == false ]] || args+=(--experimental-four-timers)
    if ! bash "$single" "${args[@]}" >"$tmp/launch-check.json" 2>"$tmp/error"; then
      reason=launch_preflight_failed
    elif ! jq -e --argjson timers "$timers" '
      type == "object" and
      .operation == "read_only_registered_bottles_profile_launch_preflight" and
      .source_game_policy_verified == true and
      .profile_file_matching_source_verified == true and
      .bottles_environment_policy_preflight_passed == true and
      .registered_program_environment_preflight_passed == true and
      .mangohud_managed_config_checksum_verified == true and
      .mangohud_managed_config_auto_selected == true and
      .experimental_startup_timer_activation_requested == $timers and
      .bottles_run_invoked == false and
      .game_process_started_verified == false and
      .bottle_configuration_modified == false and
      .attachment_attempted == false
    ' "$tmp/launch-check.json" >/dev/null 2>"$tmp/error"; then
      reason=unexpected_launch_preflight_report
    fi
  fi
  if [[ -z "$reason" ]]; then
    ready=$((ready+1))
    jq -nc --arg dir "$game" --arg bottle "$bottle" \
      --arg renderer "$renderer" --argjson timers "$timers" \
      '{operation:"game_live_qa_preflight",game_dir:$dir,bottle:$bottle,
        renderer:$renderer,experimental_timer_preflight_requested:$timers,
        status:"ready_for_live_qa",game_started:false,
        custom_hud_rendering_verified:false,gameplay_speed_verified:false}'
  else
    blocked=$((blocked+1))
    jq -nc --arg dir "$game" --arg bottle "$bottle" \
      --arg renderer "$renderer" --arg why "$reason" \
      '{operation:"game_live_qa_preflight",game_dir:$dir,bottle:$bottle,
        renderer:$renderer,status:"blocked",blocker:$why,
        game_started:false,custom_hud_rendering_verified:false,
        gameplay_speed_verified:false}'
    # Preserve a concise diagnostic without corrupting machine-readable
    # stdout. Never treat one failure as permission to skip other games.
    if [[ -s "$tmp/error" ]]; then
      printf 'BLOCKED %s: %s\n' "$game" "$(tail -n 1 "$tmp/error")" >&2
    else
      printf 'BLOCKED %s: %s\n' "$game" "$reason" >&2
    fi
  fi
done < <(jq -c '.games[]' "$manifest")

jq -nc --argjson total "$total" --argjson ready "$ready" \
  --argjson blocked "$blocked" \
  '{operation:"live_qa_preflight_summary",selected_games:$total,
    ready_for_live_qa:$ready,blocked_games:$blocked,
    game_started:false,custom_hud_rendering_verified:false,
    gameplay_speed_verified:false}'
[[ "$blocked" -eq 0 ]]
