#!/usr/bin/env bash
# One reviewed plan/apply for native Bottles env, DLL, HUD and launch profiles.
# Component ownership remains independent; no cross-component atomicity claimed.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-full-setup.sh plan MANIFEST ABS_MANAGER ABS_SOURCE_POLICY [--all-bottles]
  bash scripts/bottles-full-setup.sh status MANIFEST ABS_MANAGER [--all-bottles]
  bash scripts/bottles-full-setup.sh apply MANIFEST ABS_MANAGER ABS_SOURCE_POLICY [--runtime ABS_X64_DLL] [--all-bottles] --offline-single-player --games-closed --bottles-closed

Use an explicitly reviewed version-1 manifest from bottles-discover.sh.
Each selected game must have an explicit renderer (auto|opengl|vulkan).
No game is launched and no desktop shortcut is installed by this script.
If profile preparation fails after Bottles setup, the setup journal remains
available for explicit recovery, not destructive automatic rollback.
EOF
  exit 2
}

(( $# >= 3 )) || usage
action="$1"; manifest="$2"; manager="$3"; shift 3
[[ "$action" == plan || "$action" == apply || "$action" == status ]] || usage
source=""
if [[ "$action" != status ]]; then
  (( $# >= 1 )) || usage
  source="$1"; shift
fi
for p in "$manifest" "$manager"; do
  [[ "$p" == /* && -f "$p" && ! -L "$p" ]] || usage
done
if [[ "$action" != status ]]; then
  [[ "$source" == /* && -f "$source" && ! -L "$source" ]] || usage
fi
[[ -x "$manager" ]] || usage
for cmd in jq sha256sum realpath bottles-cli flock stat; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 2; }
done
all=false; runtime=""; offline=false; games_closed=false; bottles_closed=false
while (( $# )); do
  case "$1" in
    --all-bottles)
      [[ "$all" == false ]] || usage
      all=true; shift ;;
    --runtime)
      [[ "$action" == apply && -z "$runtime" && $# -ge 2 &&
         "$2" == /* && -f "$2" && ! -L "$2" ]] || usage
      runtime="$2"; shift 2 ;;
    --offline-single-player)
      [[ "$action" == apply && "$offline" == false ]] || usage
      offline=true; shift ;;
    --games-closed)
      [[ "$action" == apply && "$games_closed" == false ]] || usage
      games_closed=true; shift ;;
    --bottles-closed)
      [[ "$action" == apply && "$bottles_closed" == false ]] || usage
      bottles_closed=true; shift ;;
    *) usage ;;
  esac
done
if [[ "$action" == apply ]]; then
  [[ "$offline" == true && "$games_closed" == true &&
     "$bottles_closed" == true ]] || usage
fi

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
base="$here/scripts/bottles-setup.sh"
profiles="$here/scripts/bottles-profiles-batch.sh"
[[ -f "$base" && -f "$profiles" && ! -L "$base" && ! -L "$profiles" ]] ||
  { echo "Missing reviewed Intimatr components" >&2; exit 2; }

umask 077
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-bottles-full.XXXXXXXX")"
stage=preflight
journal=""
cleanup() {
  local rc=$?
  trap - EXIT
  if (( rc != 0 )) && [[ "$action" == apply ]]; then
    echo "INCOMPLETE combined setup at stage: $stage" >&2
    if [[ -n "$journal" ]]; then
      echo "Earlier Bottles setup journal: $journal" >&2
      echo "Inspect using scripts/bottles-setup-recover.sh status JOURNAL ABS_MANAGER before any explicit recovery." >&2
    else
      echo "Inspect component status; any new journal is reported by bottles-setup.sh." >&2
    fi
    echo "Combined operation is not atomic; this wrapper never guesses which owned changes to delete." >&2
  fi
  rm -rf -- "$tmp"
  exit "$rc"
}
trap cleanup EXIT

cat -- "$manifest" >"$tmp/reviewed.json" ||
  { echo "Unable to snapshot original reviewed manifest" >&2; exit 1; }
manifest="$tmp/reviewed.json"
if [[ "$action" != status ]]; then
  source_hash="$(sha256sum <"$source" | cut -d' ' -f1)"
fi
source_unchanged() {
  [[ "$(sha256sum <"$source" | cut -d' ' -f1)" == "$source_hash" ]] ||
    { echo "Source speed policy changed during combined setup; refusing mixed policy" >&2; return 1; }
}
base_args=()
[[ "$all" == false ]] || base_args+=(--all-bottles)
if [[ "$action" == status ]]; then
  echo "=== Base Bottles / DLL / HUD status ==="
  bash "$base" status "$manifest" "$manager" "${base_args[@]}"
  echo "=== Per-game launch profile status ==="
  bash "$profiles" status "$manifest"
  exit 0
fi

echo "=== Complete read-only policy and registration preflight ==="
bash "$profiles" plan "$manifest" "$manager" "$source"
source_unchanged
echo "=== Base environment / DLL / HUD preflight ==="
bash "$base" plan "$manifest" "$manager" "${base_args[@]}"
source_unchanged
if [[ "$action" == plan ]]; then
  echo "Combined plan complete: no selected game or Bottles configuration modified."
  exit 0
fi

# An earlier combined profile stage may be unfinished even when the base
# environment/DLL/HUD journal has already reached "complete". Do not layer
# another combined apply on top of an unresolved profile recovery attempt.
root_raw="$(bottles-cli --json info bottles-path)"
[[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
  { echo "Invalid native Bottles root" >&2; exit 1; }
root="$(realpath -e -- "$root_raw")"
# Combined setup/recovery needs a lock spanning the base journal AND game
# profile stage. This is separate from the base installer's root-directory
# lock, which the child process acquires while applying its own changes.
# Only explicit apply creates this inert, private file; plan/status do not.
full_lock="$root/.intimatr-full-operation.lock"
if [[ ! -e "$full_lock" && ! -L "$full_lock" ]]; then
  ( set -C; umask 077; : >"$full_lock" ) ||
    { echo "Cannot create a new private combined-operation lock" >&2; exit 1; }
fi
[[ -f "$full_lock" && ! -L "$full_lock" &&
   "$(stat -c %u -- "$full_lock")" == "$EUID" &&
   "$(stat -c %a -- "$full_lock")" == 600 &&
   "$(stat -c %h -- "$full_lock")" == 1 ]] ||
  { echo "Combined-operation lock is foreign, linked or unsafe" >&2; exit 1; }
exec 8<"$full_lock"
flock -n 8 ||
  { echo "Another Intimatr combined setup or recovery is active" >&2; exit 1; }

for previous in "$root"/.intimatr-setup.*; do
  [[ -e "$previous" || -L "$previous" ]] || continue
  if [[ -e "$previous/full-profile-stage" || -L "$previous/full-profile-stage" ]]; then
    [[ -f "$previous/full-profile-stage" && ! -L "$previous/full-profile-stage" ]] ||
      { echo "Unsafe prior profile stage marker: $previous" >&2; exit 1; }
    prior_stage="$(cat -- "$previous/full-profile-stage")"
    case "$prior_stage" in
      profiles-complete|reverted) ;;
      *)
        echo "Unfinished combined profile stage ($prior_stage) at $previous. Inspect/recover before applying again." >&2
        exit 1 ;;
    esac
  fi
done

# Record precisely which reviewed games already had a managed launch
# profile BEFORE any component mutation. This before-state is needed to
# recover newly created profiles without touching a pre-existing one.
: >"$tmp/profiles-before.jsonl"
while IFS= read -r game; do
  present=false
  if [[ -e "$game/.intimatr-managed-launch" ||
        -L "$game/.intimatr-managed-launch" ]]; then
    present=true
  fi
  jq -nc --arg game_dir "$game" --argjson preexisting "$present" \
    '{game_dir:$game_dir,preexisting:$preexisting}' >>"$tmp/profiles-before.jsonl"
done < <(jq -r '.games[].game_dir' "$manifest")
jq -s '{version:1,games:.}' "$tmp/profiles-before.jsonl" >"$tmp/profiles-before.json"
before_state_unchanged() {
  local record game was present
  while IFS= read -r record; do
    game="$(jq -er '.game_dir' <<<"$record")"
    was="$(jq -r '.preexisting' <<<"$record")"
    present=false
    if [[ -e "$game/.intimatr-managed-launch" ||
          -L "$game/.intimatr-managed-launch" ]]; then
      present=true
    fi
    [[ "$was" == "$present" ]] ||
      { echo "Managed profile existence changed after preflight: $game" >&2; return 1; }
  done < <(jq -c '.games[]' "$tmp/profiles-before.json")
}

# Existing base apply checks x64 runtime/build prerequisites before changing
# any environment allowlist. Failure/partial setup is journaled there.
stage=base-apply
apply_args=("${base_args[@]}" --offline-single-player --games-closed --bottles-closed)
[[ -z "$runtime" ]] || apply_args+=(--runtime "$runtime")
source_unchanged
bash "$base" apply "$manifest" "$manager" "${apply_args[@]}" | tee "$tmp/base-output.log"
journal="$(sed -n 's/^Recovery journal: //p' "$tmp/base-output.log" | tail -n 1)"
[[ "$journal" == /* && -d "$journal" && ! -L "$journal" &&
   "$(basename -- "$journal")" == .intimatr-setup.* &&
   -f "$journal/selection.json" && -f "$journal/stage" &&
   "$(cat -- "$journal/stage")" == complete ]] ||
  { echo "Missing or invalid completed base setup journal" >&2; exit 1; }
source_unchanged
before_state_unchanged
# Preserve a separate profile before-state in the base journal. The base
# immutable selection and environment inventory remain intact.
[[ ! -e "$journal/full-profiles-before.json" &&
   ! -e "$journal/full-profiles-before.sha256" &&
   ! -e "$journal/full-profile-stage" ]] ||
  { echo "Base setup journal already contains a profile recovery record" >&2; exit 1; }
cp -- "$tmp/profiles-before.json" "$journal/full-profiles-before.json"
(
  cd -- "$journal"
  sha256sum -- full-profiles-before.json >full-profiles-before.sha256
)
mark_full_stage() {
  printf '%s\n' "$1" >"$journal/.full-profile-stage-next"
  mv -- "$journal/.full-profile-stage-next" "$journal/full-profile-stage"
}
mark_full_stage profiles-started
stage=profile-preparation
bash "$profiles" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed
source_unchanged
# Freeze the exact completed profile identities, including the inode of each
# private managed directory. A user can remove/recreate a valid Intimatr
# profile after setup; that later profile must not be attributed to this
# attempt merely because it passes the generic ownership checksum.
[[ ! -e "$journal/full-profiles-after.json" &&
   ! -e "$journal/full-profiles-after.sha256" ]] ||
  { echo "Combined journal already contains a profile after-state" >&2; exit 1; }
: >"$tmp/profiles-after.jsonl"
while IFS= read -r game; do
  managed="$game/.intimatr-managed-launch"
  [[ -d "$managed" && ! -L "$managed" &&
     -f "$managed/snapshots.sha256" && ! -L "$managed/snapshots.sha256" ]] ||
    { echo "Incomplete or replaced managed profile after batch: $game" >&2; exit 1; }
  bash "$here/scripts/bottles-managed-launch.sh" status "$game" >/dev/null ||
    { echo "Cannot verify completed managed profile: $game" >&2; exit 1; }
  ownership_hash="$(sha256sum <"$managed/snapshots.sha256" | cut -d' ' -f1)"
  directory_identity="$(stat -c '%d:%i' -- "$managed")"
  jq -nc --arg game_dir "$game" --arg snapshot_sha256 "$ownership_hash" \
    --arg directory_identity "$directory_identity" \
    '{game_dir:$game_dir,snapshot_sha256:$snapshot_sha256,
      directory_identity:$directory_identity}' >>"$tmp/profiles-after.jsonl"
done < <(jq -r '.games[].game_dir' "$manifest")
jq -s '{version:1,games:.}' "$tmp/profiles-after.jsonl" \
  >"$journal/.full-profiles-after-next"
mv -- "$journal/.full-profiles-after-next" "$journal/full-profiles-after.json"
(
  cd -- "$journal"
  sha256sum -- full-profiles-after.json >.full-profiles-after-sha-next
  mv -- .full-profiles-after-sha-next full-profiles-after.sha256
)
mark_full_stage profiles-complete
stage=complete
echo "Combined setup complete: environments, selected DLLs/HUD and all reviewed launch profiles."
echo "No Wine process or game launched. Live overlay and speed still require QA."
