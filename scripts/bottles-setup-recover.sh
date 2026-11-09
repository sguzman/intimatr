#!/usr/bin/env bash
# Read-only inspection or ownership-checked recovery of a durable setup attempt.
# The journal is an inventory, not permission to delete an unmanaged user file.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-setup-recover.sh status ABSOLUTE_JOURNAL_DIR ABSOLUTE_MANAGER
  bash scripts/bottles-setup-recover.sh rollback ABSOLUTE_JOURNAL_DIR ABSOLUTE_MANAGER --offline-single-player --games-closed --bottles-closed
Rollback only restores Intimatr-owned files newly configured in that attempt.
A changed file, incomplete proxy, invalid journal, or conflicting bottle state
fails closed. Journal directories are printed by bottles-setup.sh apply.
EOF
  exit 2
}

[[ $# -ge 3 ]] || usage
action="$1"; journal="$2"; manager="$3"; shift 3
[[ "$action" == status || "$action" == rollback ]] || usage
[[ "$journal" == /* && -d "$journal" && ! -L "$journal" ]] || usage
[[ "$manager" == /* && -f "$manager" && -x "$manager" && ! -L "$manager" ]] || usage
if [[ "$action" == status ]]; then
  (( $# == 0 )) || usage
else
  [[ $# == 3 && "$1" == --offline-single-player &&
    "$2" == --games-closed && "$3" == --bottles-closed ]] || usage
fi
for cmd in jq realpath sha256sum cmp bottles-cli; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 2; }
done
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
hud="$here/scripts/bottles-hud.sh"
[[ -f "$hud" ]] || { echo "Missing Intimatr HUD installer" >&2; exit 2; }

root_raw="$(bottles-cli --json info bottles-path)"
[[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
  { echo "Invalid native Bottles path" >&2; exit 1; }
root="$(realpath -e -- "$root_raw")"
[[ "$(realpath -e -- "$journal")" == "$journal" && "$(dirname -- "$journal")" == "$root" &&
  "$(basename -- "$journal")" =~ ^\.intimatr-setup\.[A-Za-z0-9]{8}$ ]] ||
  { echo "Journal is not a direct, canonical Intimatr child of the native Bottles root" >&2; exit 1; }
[[ "$(stat -c %u -- "$journal")" == "$EUID" &&
   "$(stat -c %a -- "$journal")" == 700 ]] ||
  { echo "Journal permissions or owner are unsafe" >&2; exit 1; }
for file in metadata.json selection.json new-dll.json new-hud.json env-before.jsonl snapshots.sha256 stage; do
  [[ -f "$journal/$file" && ! -L "$journal/$file" ]] ||
    { echo "Missing or linked journal file: $file" >&2; exit 1; }
done
# The selected recovery target and its before-state inventory are frozen at
# setup time. Verify exact bytes, in fixed filename order, without trusting
# paths or commands contained in the checksum file itself. Stage and
# rollback-source-stage remain mutable atomic markers.
(
  cd -- "$journal"
  sha256sum -- metadata.json selection.json new-dll.json new-hud.json \
    env-before.jsonl | cmp -s - snapshots.sha256
) || { echo "BLOCKED: Journal snapshot checksum mismatch; refusing recovery" >&2; exit 1; }
jq -e --arg root "$root" \
  'type == "object" and .version == 1 and .root == $root and
   (.all_bottles | type == "boolean")' "$journal/metadata.json" >/dev/null ||
  { echo "Journal metadata does not match native Bottles root" >&2; exit 1; }
for file in selection.json new-dll.json new-hud.json; do
  jq -e 'type == "object" and .version == 1 and
         (.games | type == "array" and length <= 32) and
         all(.games[]; .game_dir | type == "string") and
         all(.games[]; .executable | type == "string")' "$journal/$file" >/dev/null ||
    { echo "Invalid journal selection: $file" >&2; exit 1; }
done
# Recovery may only target a subset of the reviewed original manifest.
for file in new-dll.json new-hud.json; do
  jq -e --slurpfile reviewed "$journal/selection.json" \
    'all(.games[]; . as $target |
      any($reviewed[0].games[]; .game_dir == $target.game_dir and
         .executable == $target.executable))' "$journal/$file" >/dev/null ||
    { echo "Journal candidate not present in original reviewed selection: $file" >&2; exit 1; }
done
jq -s -e 'all(.[]; .operation == "status" and (.bottle | type == "string") and
  (.ownership | type == "string") and (.allowlist_entries_to_add | type == "array")) and
  (map(.bottle) | length == (unique | length))' "$journal/env-before.jsonl" >/dev/null ||
  { echo "Invalid or duplicate journal environment entries" >&2; exit 1; }
stage="$(cat -- "$journal/stage")"
case "$stage" in
  prepared|environment-started|environment-complete|dll-started|dll-complete|hud-started|hud-complete|complete|rollback-started|reverted) ;;
  *) echo "Unknown journal stage: $stage" >&2; exit 1 ;;
esac
# A "started" stage may have written only some files. Do not attribute
# ownership to this attempt until that component reached its complete marker.
# An interrupted rollback retains its original stage in an independent
# marker, so a retry can distinguish already-removed files from untouched
# components. Legacy interrupted rollbacks without this marker fail closed.
recovery_source_stage="$stage"
if [[ "$stage" == rollback-started ]]; then
  [[ -f "$journal/rollback-source-stage" && ! -L "$journal/rollback-source-stage" ]] ||
    { echo "BLOCKED: interrupted rollback lacks original stage; manual inspection required" >&2; exit 1; }
  recovery_source_stage="$(cat -- "$journal/rollback-source-stage")"
fi
environment_phase=unstarted; dll_phase=unstarted; hud_phase=unstarted
case "$recovery_source_stage" in
  prepared) ;;
  environment-started) environment_phase=started ;;
  environment-complete) environment_phase=complete ;;
  dll-started) environment_phase=complete; dll_phase=started ;;
  dll-complete) environment_phase=complete; dll_phase=complete ;;
  hud-started) environment_phase=complete; dll_phase=complete; hud_phase=started ;;
  hud-complete|complete)
    environment_phase=complete; dll_phase=complete; hud_phase=complete ;;
  reverted)
    # Status is supported; rollback itself will refuse this stage below.
    environment_phase=complete; dll_phase=complete; hud_phase=complete ;;
  *) echo "BLOCKED: invalid recovery source stage: $recovery_source_stage" >&2; exit 1 ;;
esac

# Hold the same advisory lock as setup throughout rollback preflight AND
# mutation, so two Intimatr operations cannot change ownership while the
# recovery selection is being computed. Read-only status takes no lock.
if [[ "$action" == rollback ]]; then
  command -v flock >/dev/null ||
    { echo "Missing flock for exclusive rollback" >&2; exit 2; }
  exec 9<"$root"
  flock -n 9 ||
    { echo "Another Intimatr setup or rollback holds the Bottles lock" >&2; exit 1; }
fi

temp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-setup-recovery.XXXXXXXX")"
trap 'rm -rf -- "$temp"' EXIT
: >"$temp/remove-hud.jsonl"
: >"$temp/remove-dll.jsonl"
: >"$temp/restore-env.txt"
blockers=0

# Refuse to touch an installed resource not verifiably owned and unchanged.
# Absent resources are already restored and are safely skipped.
check_selected() {
  local file="$1" kind="$2" game exe state response phase
  if [[ "$kind" == hud ]]; then phase="$hud_phase"; else phase="$dll_phase"; fi
  while IFS=$'\t' read -r game exe; do
    [[ "$game" == "$root/"* && -d "$game" && ! -L "$game" &&
       "$(realpath -e -- "$game")" == "$game" ]] || {
      echo "BLOCKED: selected game is no longer an unchanged native path: $game" >&2
      blockers=$((blockers + 1))
      continue
    }
    if [[ "$kind" == hud ]]; then
      jq -nc --arg game_dir "$game" --arg executable "$exe" \
        '{version:1,games:[{game_dir:$game_dir,executable:$executable}]}' >"$temp/one.json"
      response="$(bash "$hud" status "$temp/one.json")" || {
        echo "BLOCKED: cannot inspect HUD in $game" >&2; blockers=$((blockers + 1)); continue;
      }
      if grep -Fxq -- "HUD owned (unchanged): $game" <<<"$response"; then
        if [[ "$phase" != complete ]]; then
          echo "BLOCKED: HUD stage was $phase; cannot attribute installed HUD to this attempt: $game" >&2
          blockers=$((blockers + 1))
        else
          jq -nc --arg game_dir "$game" --arg executable "$exe" \
            '{game_dir:$game_dir,executable:$executable}' >>"$temp/remove-hud.jsonl"
        fi
      elif ! grep -Fxq -- "HUD absent: $game" <<<"$response"; then
        echo "BLOCKED: HUD is modified, incomplete, or unmanaged in $game" >&2
        blockers=$((blockers + 1))
      fi
    else
      response="$("$manager" launch-proxy status --game-dir "$game")" || {
        echo "BLOCKED: cannot inspect proxy in $game" >&2; blockers=$((blockers + 1)); continue;
      }
      state="$(jq -r '.status // "invalid"' <<<"$response")"
      if [[ "$state" == installed ]] &&
          jq -e '.original_preserved == true' <<<"$response" >/dev/null; then
        if [[ "$phase" != complete ]]; then
          echo "BLOCKED: DLL stage was $phase; cannot attribute installed proxy to this attempt: $game" >&2
          blockers=$((blockers + 1))
        else
          jq -nc --arg game_dir "$game" --arg executable "$exe" \
            '{game_dir:$game_dir,executable:$executable}' >>"$temp/remove-dll.jsonl"
        fi
      elif [[ "$state" != not_installed ]]; then
        echo "BLOCKED: DLL is modified, incomplete, or unmanaged in $game" >&2
        blockers=$((blockers + 1))
      fi
    fi
  done < <(jq -r '.games[] | [.game_dir, .executable] | @tsv' "$journal/$file")
}
check_selected new-hud.json hud
check_selected new-dll.json dll

# Only restore environment entries that were unmanaged BEFORE this attempt
# and actually required additions. Pre-existing owned entries are preserved.
while IFS=$'\t' read -r bottle ownership missing; do
  [[ "$ownership" == unmanaged && "$missing" != 0 ]] || continue
  response="$("$manager" bottle-env status --bottle "$bottle" --preset intimatr-hud)" || {
    echo "BLOCKED: cannot inspect original bottle: $bottle" >&2
    blockers=$((blockers + 1)); continue;
  }
  current="$(jq -r '.ownership // "invalid"' <<<"$response")"
  case "$current" in
    owned_unchanged)
      if [[ "$environment_phase" != complete ]]; then
        echo "BLOCKED: environment stage was $environment_phase; cannot attribute managed bottle to this attempt: $bottle" >&2
        blockers=$((blockers + 1))
      else
        printf '%s\n' "$bottle" >>"$temp/restore-env.txt"
      fi ;;
    unmanaged) ;;
    *) echo "BLOCKED: environment ownership is $current for $bottle" >&2
       blockers=$((blockers + 1)) ;;
  esac
done < <(jq -r '.[] | [.bottle,.ownership,(.allowlist_entries_to_add|length)] | @tsv' \
  <(jq -s '.' "$journal/env-before.jsonl"))

hud_count="$(wc -l <"$temp/remove-hud.jsonl")"
dll_count="$(wc -l <"$temp/remove-dll.jsonl")"
env_count="$(wc -l <"$temp/restore-env.txt")"
echo "Journal: $journal"
echo "Stage: $stage"
echo "Restorable newly owned entries: $hud_count HUD, $dll_count DLL, $env_count bottle environments. Blockers: $blockers."
if [[ "$action" == status ]]; then
  echo "Status is read-only; no game or Bottles settings changed."
  exit 0
fi
[[ "$stage" != reverted ]] || { echo "Journal already rolled back" >&2; exit 2; }
(( blockers == 0 )) || { echo "Refusing partial rollback while blockers remain" >&2; exit 1; }
# Stage markers leave an audit trail. State validation happens
# again inside each installer, which refuses changes made after this preflight.
mark_stage() {
  printf '%s\n' "$1" >"$journal/.stage-next"
  mv -- "$journal/.stage-next" "$journal/stage"
}
# Keep the source stage even if a signal/power loss interrupts rollback.
# A retry rechecks ownership and skips files already restored.
if [[ "$stage" != rollback-started ]]; then
  printf '%s\n' "$stage" >"$journal/.rollback-source-stage-next"
  mv -- "$journal/.rollback-source-stage-next" "$journal/rollback-source-stage"
fi
mark_stage rollback-started
if (( hud_count )); then
  jq -s '{version:1,games:.}' "$temp/remove-hud.jsonl" >"$temp/hud.json"
  bash "$hud" remove "$temp/hud.json" --offline-single-player --games-closed
fi
if (( dll_count )); then
  jq -s '{version:1,games:.}' "$temp/remove-dll.jsonl" >"$temp/dll.json"
  # Per-game status was checked above. The installer checks it again.
  bash "$here/scripts/bottles-multi-game.sh" remove "$temp/dll.json" "$manager" \
    --offline-single-player --games-closed
fi
while IFS= read -r bottle; do
  "$manager" bottle-env restore --bottle "$bottle" --bottles-closed
done <"$temp/restore-env.txt"
mark_stage reverted
echo "Recovered the unchanged, newly owned setup entries. Journal retained for audit."
