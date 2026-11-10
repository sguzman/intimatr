#!/usr/bin/env bash
# One explicitly selected native Bottles game, one controlled live QA launch.
# No implicit bulk game launches or writes to a game's configuration.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-qa-session.sh plan ABS_REVIEWED_MANIFEST ABS_GAME_DIR --offline-single-player [--experimental-four-timers]
  bash scripts/bottles-qa-session.sh launch ABS_REVIEWED_MANIFEST ABS_GAME_DIR --offline-single-player [--experimental-four-timers]

plan: read-only; requires this one selected game to pass all launch checks.
launch: repeats preflight then explicitly starts only that reviewed game.
The game must have a prepared Intimatr-managed launch profile.
A successful Bottles CLI invocation does NOT verify responsive gameplay,
DLL load, custom HUD rendering, or speed. Test those separately in-game.
EOF
  exit 2
}
(( $# >= 3 )) || usage
action="$1"; manifest="$2"; game="$3"; shift 3
[[ "$action" == plan || "$action" == launch ]] || usage
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" &&
   "$game" == /* && -d "$game" && ! -L "$game" &&
   "$(realpath -e -- "$game")" == "$game" ]] || usage

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
command -v jq >/dev/null ||
  { echo "Missing required command: jq" >&2; exit 2; }
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
checker="$here/scripts/bottles-qa-preflight.sh"
launcher="$here/scripts/bottles-managed-launch.sh"
for script in "$checker" "$launcher"; do
  [[ -f "$script" && ! -L "$script" ]] ||
    { echo "Missing trusted Intimatr QA component: $script" >&2; exit 2; }
done

umask 077
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-qa-session.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
cp -- "$manifest" "$tmp/reviewed.json"
jq -ce --arg dir "$game" '
  if type != "object" or .version != 1 or (.games | type) != "array"
     or (.games | length) == 0 or (.games | length) > 32
     or ([.games[].game_dir] | length) != ([.games[].game_dir] | unique | length)
  then error("Not a distinct version-1 reviewed selection")
  else [.games[] | select(.game_dir == $dir)] as $matches
    | if ($matches | length) != 1 then
        error("Exactly one reviewed game directory must match")
      else $matches[0] end
  end
' "$tmp/reviewed.json" >"$tmp/selected.json" ||
  { echo "Selected game is absent or duplicated in reviewed manifest" >&2; exit 1; }

# Freeze JUST this reviewed item for static preflight. This intentionally
# does not let a not-yet-prepared second game block testing the first one,
# and does not silently authorize launch of any other game from the manifest.
jq -n --slurpfile selected "$tmp/selected.json" \
  '{version:1,games:[$selected[0]]}' >"$tmp/single-game.json"
check_args=("$tmp/single-game.json" --offline-single-player)
[[ "$timers" == false ]] || check_args+=(--experimental-four-timers)
if ! bash "$checker" "${check_args[@]}" >"$tmp/check.jsonl" 2>"$tmp/check.err"; then
  cat -- "$tmp/check.err" >&2
  echo "BLOCKED: reviewed game did not pass all static launch checks: $game" >&2
  exit 1
fi
jq -s -e --arg dir "$game" --argjson timers "$timers" '
  length == 2 and
  .[0].operation == "game_live_qa_preflight" and
  .[0].game_dir == $dir and
  .[0].status == "ready_for_live_qa" and
  .[0].experimental_timer_preflight_requested == $timers and
  .[0].game_started == false and
  .[1].operation == "live_qa_preflight_summary" and
  .[1].selected_games == 1 and
  .[1].ready_for_live_qa == 1 and
  .[1].blocked_games == 0 and
  .[1].game_started == false
' "$tmp/check.jsonl" >/dev/null ||
  { echo "BLOCKED: invalid one-game QA summary; refusing launch" >&2; exit 1; }

if [[ "$action" == plan ]]; then
  jq -nc --arg dir "$game" \
    --arg bottle "$(jq -er '.bottle' "$tmp/selected.json")" \
    --arg program "$(jq -er '.registered_program' "$tmp/selected.json")" \
    --arg renderer "$(jq -er '.renderer' "$tmp/selected.json")" \
    --argjson timers "$timers" '
    {operation:"read_only_single_game_qa_plan",game_dir:$dir,
     bottle:$bottle,registered_program:$program,renderer:$renderer,
     selected_game_static_preflight:"ready_for_live_qa",
     experimental_timers_requested_for_future_launch:$timers,
     other_games_started:false,game_started:false,
     game_responsive_verified:false,dll_loaded_verified:false,
     custom_hud_visible_verified:false,gameplay_speed_verified:false,
     can_launch_reviewed_game_explicitly:true}'
  exit 0
fi

# No implicit session backgrounding or auto-shutdown. Reuse the exact
# already audited managed launch path; it revalidates the game's profile,
# registration, and HUD immediately before invoking the native Bottles CLI.
echo "Static checks passed. Invoking only the selected reviewed offline game: $game" >&2
launch_args=(launch "$game" --offline-single-player)
[[ "$timers" == false ]] || launch_args+=(--experimental-four-timers)
bash "$launcher" "${launch_args[@]}"
