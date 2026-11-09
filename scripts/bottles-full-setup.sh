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
for cmd in jq sha256sum; do
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

# Existing base apply checks x64 runtime/build prerequisites before changing
# any environment allowlist. Failure/partial setup is journaled there.
stage=base-apply
apply_args=("${base_args[@]}" --offline-single-player --games-closed --bottles-closed)
[[ -z "$runtime" ]] || apply_args+=(--runtime "$runtime")
source_unchanged
bash "$base" apply "$manifest" "$manager" "${apply_args[@]}" | tee "$tmp/base-output.log"
journal="$(sed -n 's/^Recovery journal: //p' "$tmp/base-output.log" | tail -n 1)"
source_unchanged
stage=profile-preparation
bash "$profiles" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed
source_unchanged
stage=complete
echo "Combined setup complete: environments, selected DLLs/HUD and all reviewed launch profiles."
echo "No Wine process or game launched. Live overlay and speed still require QA."
