#!/usr/bin/env bash
# Standalone read-only/temporary-fixture regression for the MangoHud adapter.
# No Wine, graphical session, game installation, sudo, or Rust build required.
set -euo pipefail

command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 2; }
tmp="$(mktemp -d)"
first="$tmp/First Game"
second="$tmp/Second Game"
cleanup() {
  chmod u+rwx "$second" 2>/dev/null || true
  rm -rf -- "$tmp"
}
trap cleanup EXIT
mkdir -p "$first" "$second"
printf 'fixture\n' >"$first/First Game.exe"
printf 'fixture\n' >"$second/Second Game.exe"
for dir in "$first" "$second"; do
  printf '{}\n' >"$dir/intimatr-launch-deployment.json"
  printf 'fixture\n' >"$dir/intimatr.dll"
done
manifest="$tmp/manifest.json"
jq -n --arg one "$first" --arg two "$second" \
  '{version:1,games:[{game_dir:$one,executable:"First Game.exe"},
                     {game_dir:$two,executable:"Second Game.exe"}]}' \
  >"$manifest"

bash -n scripts/bottles-hud.sh
bash scripts/bottles-hud.sh plan "$manifest" >"$tmp/plan.log"
test "$(grep -c '^HUD candidate: ' "$tmp/plan.log")" -eq 2
bash scripts/bottles-hud.sh status "$manifest" >"$tmp/absent.log"
test "$(grep -c '^HUD absent: ' "$tmp/absent.log")" -eq 2

# The first install succeeds, then the second is made unwritable. The entire
# attempt must fail with the first game's unchanged HUD rolled back.
chmod 0555 "$second"
if bash scripts/bottles-hud.sh install "$manifest" \
    --offline-single-player --games-closed >"$tmp/failure.out" 2>"$tmp/failure.err"; then
  echo "ERROR: installation succeeded in read-only second directory" >&2
  exit 1
fi
chmod 0755 "$second"
grep -q 'HUD batch install failed' "$tmp/failure.err"
grep -Fq "Rolled back unchanged HUD config: $first" "$tmp/failure.err"
for dir in "$first" "$second"; do
  test ! -e "$dir/MangoHud.conf"
  test ! -e "$dir/intimatr-mangohud.sha256"
  test -f "$dir/intimatr.dll"
done

bash scripts/bottles-hud.sh install "$manifest" \
  --offline-single-player --games-closed >"$tmp/installed.log"
bash scripts/bottles-hud.sh status "$manifest" >"$tmp/owned.log"
test "$(grep -c '^HUD owned (unchanged): ' "$tmp/owned.log")" -eq 2

for dir in "$first" "$second"; do
  grep -Fxq fps "$dir/MangoHud.conf"
  grep -Fxq cpu_stats "$dir/MangoHud.conf"
  grep -Fxq gpu_stats "$dir/MangoHud.conf"
  grep -Fxq ram "$dir/MangoHud.conf"
  grep -Fxq vram "$dir/MangoHud.conf"
  exec_command="$(sed -n 's/^exec=//p' "$dir/MangoHud.conf")"
  test "$(bash -c "$exec_command")" = '--'
  printf '2.00x\n' >"$dir/intimatr-hud-rate.txt"
  test "$(bash -c "$exec_command")" = '2.00x'
  touch -d '12 seconds ago' "$dir/intimatr-hud-rate.txt"
  test "$(bash -c "$exec_command")" = '--'
  rm -f -- "$dir/intimatr-hud-rate.txt"
done

# A locally customized second HUD must veto removal of the whole batch.
cp -- "$second/MangoHud.conf" "$tmp/second-original.conf"
printf '# local customization\n' >>"$second/MangoHud.conf"
if bash scripts/bottles-hud.sh remove "$manifest" \
    --offline-single-player --games-closed >"$tmp/tampered.out" 2>"$tmp/tampered.err"; then
  echo "ERROR: locally edited HUD was removed" >&2
  exit 1
fi
grep -q 'MangoHud config was modified' "$tmp/tampered.err"
test -f "$first/MangoHud.conf"
test -f "$second/MangoHud.conf"
cp -- "$tmp/second-original.conf" "$second/MangoHud.conf"
# Deleting only mock proxy files must not block independent HUD cleanup.
for dir in "$first" "$second"; do
  rm -f -- "$dir/intimatr.dll" "$dir/intimatr-launch-deployment.json"
done
bash scripts/bottles-hud.sh remove "$manifest" \
  --offline-single-player --games-closed >"$tmp/removed.log"
for dir in "$first" "$second"; do
  test ! -e "$dir/MangoHud.conf"
  test ! -e "$dir/intimatr-mangohud.sha256"
done
echo "HUD shell fixture: two-game rollback, unchanged ownership, stale-rate fallback, and cleanup verified"
