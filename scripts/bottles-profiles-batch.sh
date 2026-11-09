#!/usr/bin/env bash
# Explicit bulk preparation for reviewed offline Bottles game launch profiles.
# Uses the existing single-game ownership checks; does not install DLL/HUD.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-profiles-batch.sh plan MANIFEST ABSOLUTE_MANAGER ABSOLUTE_SOURCE_POLICY
  bash scripts/bottles-profiles-batch.sh prepare MANIFEST ABSOLUTE_MANAGER ABSOLUTE_SOURCE_POLICY --offline-single-player --games-closed
  bash scripts/bottles-profiles-batch.sh status MANIFEST
  bash scripts/bottles-profiles-batch.sh remove MANIFEST --games-closed

The reviewed version-1 manifest must specify for EVERY selected game:
  game_dir, executable, bottle, registered_program, windows_executable,
  renderer: opengl|vulkan|auto
No default renderer is guessed and no extra game is enrolled.
This is best-effort compensating rollback, not a crash-durable transaction.
EOF
  exit 2
}

(( $# >= 2 )) || usage
action="$1"; manifest="$2"; shift 2
case "$action" in plan|prepare|status|remove) ;; *) usage ;; esac
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || usage
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
single="$here/scripts/bottles-managed-launch.sh"
[[ -f "$single" && ! -L "$single" ]] || { echo "Missing Intimatr managed launcher" >&2; exit 2; }
for cmd in jq realpath bottles-cli; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 2; }
done

manager=""; source=""; offline=false; closed=false
if [[ "$action" == plan || "$action" == prepare ]]; then
  (( $# >= 2 )) || usage
  manager="$1"; source="$2"; shift 2
  [[ "$manager" == /* && -x "$manager" && -f "$manager" && ! -L "$manager" &&
     "$source" == /* && -f "$source" && ! -L "$source" ]] || usage
fi
while (( $# )); do
  case "$1" in
    --offline-single-player)
      [[ "$action" == prepare && "$offline" == false ]] || usage
      offline=true; shift ;;
    --games-closed)
      [[ ( "$action" == prepare || "$action" == remove ) && "$closed" == false ]] || usage
      closed=true; shift ;;
    *) usage ;;
  esac
done
if [[ "$action" == prepare ]]; then
  [[ "$offline" == true && "$closed" == true ]] || usage
elif [[ "$action" == remove ]]; then
  [[ "$closed" == true ]] || usage
fi

umask 077
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-profiles-batch.XXXXXXXX")"
declare -a created=()
current=""
cleanup() {
  local rc=$?
  trap - EXIT
  if (( rc != 0 )) && [[ "$action" == prepare ]]; then
    echo "INCOMPLETE: profile batch stopped at $current; checking newly prepared profiles for safe rollback" >&2
    local i game
    for (( i=${#created[@]}-1; i>=0; i-- )); do
      game="${created[i]}"
      if bash "$single" remove "$game" --games-closed >&2; then
        echo "Restored newly prepared profile: $game" >&2
      else
        echo "MANUAL RECOVERY REQUIRED: unable to remove newly prepared profile: $game" >&2
      fi
    done
    echo "Any incomplete current game's managed directory is intentionally retained for manual inspection" >&2
  fi
  rm -rf -- "$tmp"
  exit "$rc"
}
trap cleanup EXIT

cat -- "$manifest" >"$tmp/reviewed.json" ||
  { echo "Unable to snapshot reviewed selection" >&2; exit 1; }
manifest="$tmp/reviewed.json"
jq -e '
  type == "object" and .version == 1 and
  (.games | type == "array" and length > 0 and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  all(.games[];
    type == "object" and
    (.game_dir | type == "string" and startswith("/") and
      (test("[\t\r\n\\]") | not)) and
    (.executable | type == "string" and
      test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$") and
      (contains("..") | not)) and
    (.bottle | type == "string" and length > 0 and (test("[\t\r\n]") | not)) and
    (.registered_program | type == "string" and length > 0 and (test("[\t\r\n]") | not)) and
    (.windows_executable | type == "string" and length > 0 and
      (test("[\t\r\n]") | not)) and
    (.renderer == "auto" or .renderer == "opengl" or .renderer == "vulkan")
  )
' "$manifest" >/dev/null ||
  { echo "Invalid reviewed game or missing explicit per-game renderer" >&2; exit 2; }

# Keep the frozen source manifest for all invocations. The single-game
# command verifies canonical C: identity and exact native registration.
declare -a games=() renderers=()
while IFS= read -r record; do
  games+=("$(jq -er '.game_dir' <<<"$record")")
  renderers+=("$(jq -er '.renderer' <<<"$record")")
done < <(jq -c '.games[]' "$manifest")

# Preflight EVERY selected item before the first mutation. No launch, Wine,
# DLL install, HUD rewrite, or environment change occurs in this script.
for i in "${!games[@]}"; do
  game="${games[i]}"; current="$game"
  if [[ "$action" == plan || "$action" == prepare ]]; then
    bash "$single" plan "$manifest" "$manager" "$game" "$source" \
      --renderer "${renderers[i]}"
  else
    if [[ ! -e "$game/.intimatr-managed-launch" &&
          ! -L "$game/.intimatr-managed-launch" ]]; then
      echo "Absent managed profile: $game"
    else
      bash "$single" status "$game"
    fi
  fi
done
if [[ "$action" == plan || "$action" == status ]]; then
  echo "Read-only $action: no game was launched and no files were modified."
  exit 0
fi

if [[ "$action" == remove ]]; then
  # First verify all existing owned profiles. If one is damaged, refuse the
  # entire batch BEFORE touching any other selected game.
  for game in "${games[@]}"; do
    [[ -e "$game/.intimatr-managed-launch" ||
       -L "$game/.intimatr-managed-launch" ]] || continue
    bash "$single" status "$game" >/dev/null
  done
  for game in "${games[@]}"; do
    [[ -e "$game/.intimatr-managed-launch" ||
       -L "$game/.intimatr-managed-launch" ]] || continue
    bash "$single" remove "$game" --games-closed
  done
  echo "Removed only unchanged, selected managed game profiles."
  exit 0
fi

for i in "${!games[@]}"; do
  game="${games[i]}"; current="$game"
  old=false
  [[ ! -e "$game/.intimatr-managed-launch" &&
     ! -L "$game/.intimatr-managed-launch" ]] || old=true
  bash "$single" prepare "$manifest" "$manager" "$game" "$source" \
    --renderer "${renderers[i]}" --offline-single-player --games-closed
  if [[ "$old" == false ]]; then
    # Never record an incomplete deployment as owned by this batch.
    bash "$single" status "$game" >/dev/null
    created+=("$game")
  fi
done
echo "Prepared all ${#games[@]} explicitly reviewed profiles; no game launched."
