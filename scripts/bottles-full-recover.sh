#!/usr/bin/env bash
# Explicit, ownership-checked recovery for an entire combined Bottles attempt.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-full-recover.sh status ABS_JOURNAL ABS_MANAGER
  bash scripts/bottles-full-recover.sh rollback ABS_JOURNAL ABS_MANAGER --offline-single-player --games-closed --bottles-closed
Only newly created, unchanged game profiles are eligible for removal;
the base rollback independently checks DLL, HUD and environment ownership.
An interrupted profile preparation is not evidence of completed installation.
EOF
  exit 2
}
(( $# >= 3 )) || usage
action="$1"; journal="$2"; manager="$3"; shift 3
[[ "$action" == status || "$action" == rollback ]] || usage
[[ "$journal" == /* && -d "$journal" && ! -L "$journal" &&
   "$manager" == /* && -f "$manager" && -x "$manager" && ! -L "$manager" ]] || usage
if [[ "$action" == status ]]; then
  (( $# == 0 )) || usage
else
  [[ $# == 3 && "$1" == --offline-single-player &&
     "$2" == --games-closed && "$3" == --bottles-closed ]] || usage
fi
for cmd in jq sha256sum cmp realpath bottles-cli; do
  command -v "$cmd" >/dev/null ||
    { echo "Missing prerequisite: $cmd" >&2; exit 2; }
done
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
base="$here/scripts/bottles-setup-recover.sh"
single="$here/scripts/bottles-managed-launch.sh"
[[ -f "$base" && ! -L "$base" && -f "$single" && ! -L "$single" ]] ||
  { echo "Missing Intimatr recovery scripts" >&2; exit 2; }

root_raw="$(bottles-cli --json info bottles-path)"
[[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
  { echo "Invalid native Bottles root" >&2; exit 1; }
root="$(realpath -e -- "$root_raw")"
[[ "$(realpath -e -- "$journal")" == "$journal" &&
   "$(dirname -- "$journal")" == "$root" &&
   "$(basename -- "$journal")" =~ ^\.intimatr-setup\.[A-Za-z0-9]{8}$ &&
   "$(stat -c %u -- "$journal")" == "$EUID" &&
   "$(stat -c %a -- "$journal")" == 700 ]] ||
  { echo "Invalid, foreign, or incorrectly owned recovery journal" >&2; exit 1; }

for file in selection.json full-profiles-before.json full-profiles-before.sha256 full-profile-stage stage; do
  [[ -f "$journal/$file" && ! -L "$journal/$file" ]] ||
    { echo "Missing or linked recovery file: $file" >&2; exit 1; }
done
(
  cd -- "$journal"
  sha256sum -- full-profiles-before.json | cmp -s - full-profiles-before.sha256
) || { echo "Combined recovery before-state checksum mismatch" >&2; exit 1; }

# Every profile target must be exactly one game selected by the base journal.
jq -e --slurpfile selection "$journal/selection.json" --arg root "$root" '
  type == "object" and .version == 1 and
  (.games | type == "array" and length > 0 and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  ([.games[].game_dir] | sort) == ([$selection[0].games[].game_dir] | sort) and
  all(.games[]; (.game_dir | type == "string" and startswith($root + "/")) and
    (.preexisting | type == "boolean"))
' "$journal/full-profiles-before.json" >/dev/null ||
  { echo "Combined recovery inventory differs from original reviewed selection" >&2; exit 1; }
stage="$(cat -- "$journal/full-profile-stage")"
case "$stage" in profiles-started|profiles-complete|rollback-started|reverted) ;;
  *) echo "Unknown combined profile stage: $stage" >&2; exit 1 ;;
esac
base_report="$(bash "$base" status "$journal" "$manager")" ||
  { echo "Base journal integrity or ownership inspection failed" >&2; exit 1; }
printf '%s\n' "$base_report"
base_blockers="$(sed -nE 's/^Restorable newly owned entries: .*Blockers: ([0-9]+)\.$/\1/p' <<<"$base_report")"
[[ "$base_blockers" =~ ^[0-9]+$ ]] ||
  { echo "Missing valid base rollback blocker count" >&2; exit 1; }
base_stage="$(cat -- "$journal/stage")"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-full-recovery.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
: >"$tmp/created.txt"
blockers=0; preexisting_count=0; created_count=0
while IFS= read -r record; do
  game="$(jq -er .game_dir <<<"$record")"
  preexisting="$(jq -r .preexisting <<<"$record")"
  [[ -d "$game" && ! -L "$game" &&
     "$(realpath -e -- "$game")" == "$game" ]] ||
    { echo "BLOCKED: selected game moved: $game" >&2; blockers=$((blockers+1)); continue; }
  if [[ "$preexisting" == true ]]; then
    preexisting_count=$((preexisting_count+1))
    continue
  fi
  [[ -e "$game/.intimatr-managed-launch" ||
     -L "$game/.intimatr-managed-launch" ]] || continue
  if ! bash "$single" status "$game" >"$tmp/profile-status.json"; then
    echo "BLOCKED: incomplete or user-modified profile: $game" >&2
    blockers=$((blockers+1)); continue
  fi
  if [[ "$stage" == profiles-started ]]; then
    echo "BLOCKED: cannot attribute incomplete-stage profile to this attempt: $game" >&2
    blockers=$((blockers+1)); continue
  fi
  created_count=$((created_count+1))
  printf '%s\n' "$game" >>"$tmp/created.txt"
done < <(jq -c '.games[]' "$journal/full-profiles-before.json")

echo "Combined journal: $journal"
echo "Profile stage: $stage"
echo "Eligible new profiles: $created_count; pre-existing preserved: $preexisting_count; profile blockers: $blockers; base blockers: $base_blockers."
if [[ "$action" == status ]]; then
  echo "Status is read-only; no selected game or Bottles files changed."
  exit 0
fi
if [[ "$stage" == reverted ]]; then
  echo "Combined journal already reverted."
  exit 0
fi
[[ "$stage" != profiles-started ]] ||
  { echo "BLOCKED: interrupted profile stage requires manual inspection" >&2; exit 1; }
(( blockers == 0 && base_blockers == 0 )) ||
  { echo "BLOCKED: ownership conflicts prevent combined rollback" >&2; exit 1; }

mark_stage() {
  printf '%s\n' "$1" >"$journal/.full-profile-stage-next"
  mv -- "$journal/.full-profile-stage-next" "$journal/full-profile-stage"
}
mark_stage rollback-started
while IFS= read -r game; do
  bash "$single" remove "$game" --games-closed
done <"$tmp/created.txt"
if [[ "$base_stage" != reverted ]]; then
  bash "$base" rollback "$journal" "$manager" \
    --offline-single-player --games-closed --bottles-closed
fi
mark_stage reverted
echo "Combined recovery complete; preserved pre-existing profiles and the journal."
