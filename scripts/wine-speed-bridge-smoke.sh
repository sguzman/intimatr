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
  local result=$?
  trap - EXIT
  if [[ -n "$fixture_pid" ]]; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -f "$fixture_dir"/bridge-*.flag
  # Wine helper subprocesses may outlive the fixture and still create prefix
  # files. Stop and drain only this unique disposable test prefix.
  if command -v wineserver >/dev/null 2>&1; then
    WINEPREFIX="$prefix" wineserver -k >/dev/null 2>&1 || true
    WINEPREFIX="$prefix" wineserver -w >/dev/null 2>&1 || true
  fi
  # A file race with Wine's final teardown is not a timing-test failure.
  for _ in 1 2 3 4 5; do
    if rm -rf -- "$prefix" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if [[ -d "$prefix" ]]; then
    echo "Warning: disposable Wine test prefix could not be fully removed: $prefix" >&2
  fi
  exit "$result"
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
for stage in 1 2 3 4 5; do
  case "$stage" in
    1) rate=2.0; args=(preset "$rate") ;;
    2) rate=0.5; args=(preset "$rate") ;;
    3) rate=1.0; args=(preset "$rate") ;;
    4) rate=2.0; args=(hold "$rate" 1800) ;;
    5) rate=1.0; args=(release) ;;
  esac
  "$native_cli" speed \
    --prefix "$prefix" \
    --runner "$runner" \
    --client "$client" \
    --tcp 127.0.0.1:31379 \
    "${args[@]}" > "$fixture_dir/bridge-stage-$stage.json"
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
