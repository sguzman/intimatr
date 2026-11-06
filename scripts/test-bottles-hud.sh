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

# A read-only diagnostic has no access to game processes or Vulkan frames.
bash scripts/bottles-hud.sh diagnose "$manifest" >"$tmp/diagnose-empty.json"
jq -e '.operation == "read_only_bottles_hud_diagnostics"
  and .files_modified == false
  and .in_game_rendering_verified == false
  and .gameplay_speed_verified == false
  and (.games | length) == 2
  and all(.games[]; .managed_config_state == "owned_unchanged"
    and .speed_sample_state == "missing"
    and .observed_installed_clock_rate == null)' "$tmp/diagnose-empty.json" >/dev/null
printf '2.00x\n' >"$first/intimatr-hud-rate.txt"
printf '0.50x\n' >"$second/intimatr-hud-rate.txt"
touch -d '12 seconds ago' "$second/intimatr-hud-rate.txt"
bash scripts/bottles-hud.sh diagnose "$manifest" >"$tmp/diagnose-mixed.json"
jq -e --arg first "$first" --arg second "$second" '
  ([.games[] | select(.game_dir == $first and .speed_sample_state == "fresh"
    and .observed_installed_clock_rate == "2.00x")] | length == 1)
  and ([.games[] | select(.game_dir == $second and .speed_sample_state == "expired"
    and .observed_installed_clock_rate == null)] | length == 1)'
  "$tmp/diagnose-mixed.json" >/dev/null
rm -f -- "$first/intimatr-hud-rate.txt" "$second/intimatr-hud-rate.txt"
ln -s "$tmp/diagnose-empty.json" "$first/intimatr-hud-rate.txt"
bash scripts/bottles-hud.sh diagnose "$manifest" >"$tmp/diagnose-unsafe.json"
jq -e --arg first "$first" '
  [.games[] | select(.game_dir == $first
    and .speed_sample_state == "unsafe_or_invalid"
    and .observed_installed_clock_rate == null)] | length == 1'
  "$tmp/diagnose-unsafe.json" >/dev/null
rm -- "$first/intimatr-hud-rate.txt"

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
bash scripts/bottles-hud.sh diagnose "$manifest" >"$tmp/diagnose-tampered.json"
jq -e --arg second "$second" '
  [.games[] | select(.game_dir == $second
    and .managed_config_state == "modified")] | length == 1'
  "$tmp/diagnose-tampered.json" >/dev/null
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
