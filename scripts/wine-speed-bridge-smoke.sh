#!/usr/bin/env bash
# Isolated Linux -> Wine client -> DLL RPC -> four IAT timer imports.
# CI-only disposable prefix and fixed local fixture port; no user's Bottles state.
set -euo pipefail

fixture_dir="$(pwd)/target/wine-smoke"
native_cli="$(pwd)/target/release/intimatr-bottles"
runner="$(command -v wine)"
client="$fixture_dir/intimatr-speed.exe"
prefix="$(mktemp -d "${RUNNER_TEMP:-/tmp}/intimatr-speed-bridge.XXXXXX")"
export WINEPREFIX="$prefix" WINEARCH=win64 WINEDEBUG=-all
export INTIMATR_EXTERNAL_SPEED_BRIDGE=1

for file in "$fixture_dir"/bridge-*.flag; do
  if [[ -f "$file" ]]; then rm "$file"; fi
done

fixture_pid=''
cleanup() {
  if [[ -n "$fixture_pid" ]]; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -f "$fixture_dir"/bridge-*.flag
  # Only dispose this script's unique temp prefix, never an existing bottle.
  rm -rf -- "$prefix"
}
trap cleanup EXIT

wine "$fixture_dir/wine-load-probe.exe" > "$fixture_dir/bridge-fixture.stdout" 2> "$fixture_dir/bridge-fixture.stderr" &
fixture_pid=$!

await_file() {
  local filename="$1"
  for _ in $(seq 1 600); do
    if [[ -f "$filename" ]]; then return 0; fi
    if ! kill -0 "$fixture_pid" 2>/dev/null; then
      cat "$fixture_dir/bridge-fixture.stderr" >&2
      echo "Wine fixture terminated before $filename" >&2
      return 1
    fi
    sleep 0.05
  done
  cat "$fixture_dir/bridge-fixture.stderr" >&2
  echo "timed out waiting for $filename" >&2
  return 1
}

await_file "$fixture_dir/bridge-ready.flag"
for stage in 1 2 3; do
  case "$stage" in
    1) rate=2.0 ;;
    2) rate=0.5 ;;
    3) rate=1.0 ;;
  esac
  "$native_cli" speed \
    --prefix "$prefix" \
    --runner "$runner" \
    --client "$client" \
    --tcp 127.0.0.1:31379 \
    preset "$rate" > "$fixture_dir/bridge-stage-$stage.json"
  jq -e --argjson rate "$rate" \
    '.configured == true and .preview_multiplier == $rate and .clock_applied_to_game == false' \
    "$fixture_dir/bridge-stage-$stage.json" >/dev/null
  touch "$fixture_dir/bridge-step-$stage.flag"
  await_file "$fixture_dir/bridge-ack-$stage.flag"
done

wait "$fixture_pid"
fixture_pid=''
grep -q "Linux Bottles speed CLI drove four hooked APIs through external Wine RPC" \
  "$fixture_dir/bridge-fixture.stdout"
echo "Linux-to-Wine external speed manager integration verified"
