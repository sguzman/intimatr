#!/usr/bin/env bash
# Read-only discovery of candidate registered Bottles games for manifest review.
# This NEVER installs a proxy, starts Wine, or modifies any game.
set -euo pipefail
if [[ $# != 2 || "$1" != /* || ! -x "$1" || "$2" != /* || -e "$2" || -L "$2" ]]; then
  echo "Usage: bash scripts/bottles-discover.sh ABSOLUTE_INTIMATR_BOTTLES ABSOLUTE_NEW_MANIFEST_JSON" >&2
  exit 2
fi
manager="$1"
output="$2"
for tool in jq x86_64-w64-mingw32-objdump; do
  command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 2; }
done
umask 077
temp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-bottles-discover.XXXXXXXX")"
trap 'rm -rf -- "$temp"' EXIT
"$manager" bottle-compat --all >"$temp/compat.json"
jq -e '.operation == "read_only_all_registered_bottles_static_pe_compatibility"' "$temp/compat.json" >/dev/null
# Retain exact native Bottles registration identity alongside each
# candidate. This allows a future managed launcher to select a registered
# program without guessing from the basename or hand-editing its name.
jq -c '
  .bottles[]
  | select(.bottle_prefix_association_verified == true and
      (.bottle | type == "string"))
  | . as $bottle
  | .programs[]
  | select(.registration_metadata_valid == true and
      .static_pe_compatibility.static_import_status == "static_four_timer_names_present" and
      (.resolved_linux_file | type == "string") and
      (.registered_name | type == "string" and length > 0 and
        (test("[\\t\\r\\n]") | not)) and
      (.registered_windows_path | type == "string" and length > 0 and
        (test("[\\t\\r\\n]") | not)))
  | {resolved_linux_file, bottle:$bottle.bottle,
     registered_program:.registered_name,
     windows_executable:.registered_windows_path}
' "$temp/compat.json" >"$temp/candidates.jsonl"

: >"$temp/selected.jsonl"
while IFS= read -r candidate; do
  exe="$(jq -er '.resolved_linux_file' <<<"$candidate")" || continue
  bottle="$(jq -er '.bottle' <<<"$candidate")" || continue
  program="$(jq -er '.registered_program' <<<"$candidate")" || continue
  windows_executable="$(jq -er '.windows_executable' <<<"$candidate")" || continue
  [[ "$exe" == /* && -f "$exe" && ! -L "$exe" ]] || continue
  game="$(dirname -- "$exe")"
  [[ ! -L "$game" && -f "$game/steam_api64.dll" && ! -L "$game/steam_api64.dll" ]] || continue
  filename="$(basename -- "$exe")"
  [[ "$filename" =~ ^[A-Za-z0-9_][A-Za-z0-9\ ._-]{0,123}\.[eE][xX][eE]$ && "$filename" != *..* ]] || continue
  if ! x86_64-w64-mingw32-objdump -p "$exe" |
      grep -Ei '^[[:space:]]*DLL Name: steam_api64\.dll[[:space:]]*$' >/dev/null; then
    continue
  fi
  if ! grep -Fqx "$game" "$temp/seen.txt" 2>/dev/null; then
    printf '%s\n' "$game" >>"$temp/seen.txt"
    jq -nc --arg game_dir "$game" --arg executable "$filename" \
      --arg bottle "$bottle" --arg registered_program "$program" \
      --arg windows_executable "$windows_executable" \
      '{game_dir:$game_dir,executable:$executable,bottle:$bottle,
        registered_program:$registered_program,
        windows_executable:$windows_executable,
        renderer:"auto"}' >>"$temp/selected.jsonl"
  fi
done <"$temp/candidates.jsonl"

jq -s '{version:1,games:.}' "$temp/selected.jsonl" >"$temp/manifest.json"
# Refuse overwrites and links, including races with another writer.
( set -C; cat "$temp/manifest.json" >"$output" ) || { echo "Refusing existing manifest output" >&2; exit 1; }
count="$(jq '.games | length' "$temp/manifest.json")"
echo "Wrote $count read-only candidates to $output. Review every game and exclude multiplayer/anti-cheat entries before batch install."
echo "Discovery reports only static import candidates; it does not prove gameplay timing compatibility."
