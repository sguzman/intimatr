#!/usr/bin/env bash
# Disposable Wine startup regression; does not inspect or modify real games.
set -euo pipefail
dir="$(pwd)/target/wine-launch-proxy"
mkdir -p "$dir/config"
for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-dlltool x86_64-w64-mingw32-objdump wine xvfb-run; do command -v "$tool" >/dev/null; done
cp target/x86_64-pc-windows-gnu/release/intimatr.dll "$dir/intimatr.dll"
cp config/steam_api64_launch_probe.exe.toml "$dir/config/"
x86_64-w64-mingw32-gcc -shared -o "$dir/steam_api64.dll" native/launch_proxy/steam_api64_original_fixture.c -Wl,--out-implib,"$dir/libsteam_fixture.a"
x86_64-w64-mingw32-gcc -o "$dir/steam_api64_launch_probe.exe" native/launch_proxy/steam_api64_launch_probe.c "$dir/libsteam_fixture.a" -lwinmm
mv "$dir/steam_api64.dll" "$dir/steam_api64_intimatr_original.dll"
if ! INTIMATR_OBJDUMP=x86_64-w64-mingw32-objdump bash scripts/generate-steam-api64-forwarders.sh "$dir/steam_api64_intimatr_original.dll" "$dir/forwarders.def"; then
  echo "Synthetic original PE export table for diagnosing parser mismatch:" >&2
  x86_64-w64-mingw32-objdump -p "$dir/steam_api64_intimatr_original.dll" | sed -n '/The Export Tables/,+32p' >&2
  exit 1
fi
grep -q 'IntimatrFixtureValue=steam_api64_intimatr_original.IntimatrFixtureValue @1' "$dir/forwarders.def"
printf 'LIBRARY intimatr.dll\nEXPORTS\nintimatr_lifecycle_state\n' >"$dir/intimatr_import.def"
x86_64-w64-mingw32-dlltool --def "$dir/intimatr_import.def" --dllname intimatr.dll --output-lib "$dir/libintimatr.dll.a"
x86_64-w64-mingw32-gcc -shared -o "$dir/steam_api64.dll" native/launch_proxy/steam_api64_bridge.c "$dir/forwarders.def" "$dir/libintimatr.dll.a"
x86_64-w64-mingw32-objdump -p "$dir/steam_api64.dll" >"$dir/proxy-headers.txt"
grep -q 'DLL Name: intimatr.dll' "$dir/proxy-headers.txt"
grep -q 'steam_api64_intimatr_original.IntimatrFixtureValue' "$dir/proxy-headers.txt"
prefix="$(mktemp -d /tmp/intimatr-launch-proxy.XXXXXX)"
cleanup(){
  code=$?
  trap - EXIT
  WINEPREFIX="$prefix" wineserver -k >/dev/null 2>&1 || true
  WINEPREFIX="$prefix" wineserver -w >/dev/null 2>&1 || true
  rm -rf -- "$prefix"
  exit "$code"
}
trap cleanup EXIT
WINEPREFIX="$prefix" WINEARCH=win64 WINEDEBUG=-all WINEDLLOVERRIDES='steam_api64=n' xvfb-run -a wine "$dir/steam_api64_launch_probe.exe" >"$dir/probe.stdout" 2>"$dir/probe.stderr" || {
  cat "$dir/probe.stderr" >&2
  cat "$dir/probe.stdout" >&2
  exit 1
}
grep -q 'Intimatr launch proxy startup verified' "$dir/probe.stdout"
cat "$dir/probe.stdout"
# A separate affirmative environment flag plus an accepted launch profile
# authorizes automatic four-timer arming in the owned Wine fixture only.
mkdir -p "$prefix/drive_c"
printf 'version = 1\n[speed]\nenabled = true\ninitial_multiplier = 1.0\n' >"$prefix/drive_c/intimatr-startup-qa.toml"
WINEPREFIX="$prefix" WINEARCH=win64 WINEDEBUG=-all \
  WINEDLLOVERRIDES='steam_api64=n' \
  INTIMATR_EXPERIMENTAL_STARTUP_TIMERS=1 \
  INTIMATR_SPEED_PROFILE='C:\intimatr-startup-qa.toml' \
  xvfb-run -a wine "$dir/steam_api64_launch_probe.exe" \
  >"$dir/probe-armed.stdout" 2>"$dir/probe-armed.stderr" || {
    cat "$dir/probe-armed.stderr" >&2
    cat "$dir/probe-armed.stdout" >&2
    exit 1
  }
grep -q 'Intimatr load-time startup hooks verified' "$dir/probe-armed.stdout"
cat "$dir/probe-armed.stdout"
# The user's read-only Eastward export audit is retained only as compressed
# names/ordinals, not any game code. Independently compile its full ABI surface.
base64 -d tests/fixtures/eastward-steam-forwarders.def.gz.b64 | gzip -dc >"$dir/eastward.def"
printf '%s  %s\n' '526914fdf4d850c2973ec61347d5b7a4a09dba9a2410f627f7f5ceb05c479ffd' "$dir/eastward.def" | sha256sum -c -
test "$(grep -c '=steam_api64_intimatr_original\.' "$dir/eastward.def")" -eq 1209
x86_64-w64-mingw32-gcc -shared -o "$dir/steam_api64-eastward-preview.dll" native/launch_proxy/steam_api64_bridge.c "$dir/eastward.def" "$dir/libintimatr.dll.a"
x86_64-w64-mingw32-objdump -p "$dir/steam_api64-eastward-preview.dll" >"$dir/eastward-proxy-headers.txt"
grep -q 'DLL Name: intimatr.dll' "$dir/eastward-proxy-headers.txt"
grep -q 'steam_api64_intimatr_original.GetHSteamPipe' "$dir/eastward-proxy-headers.txt"
grep -q 'steam_api64_intimatr_original.g_pSteamClientGameServer' "$dir/eastward-proxy-headers.txt"
echo "Eastward 1209-export proxy compiled and linked from text manifest; real game not modified"

# Exercise native Rust install/status/remove on a disposable Eastward-shaped
# directory, not a real game/bottle. Prove original bytes survive a round trip.
fixture="$dir/managed-fixture/Eastward/drive_c/Program Files/Eastward"
mkdir -p "$fixture"
printf 'Name: Eastward\n' >"$dir/managed-fixture/Eastward/bottle.yml"
printf 'fixture executable\n' >"$fixture/Eastward.exe"
cp "$dir/steam_api64_intimatr_original.dll" "$fixture/steam_api64.dll"
manager="target/release/intimatr-bottles"
"$manager" launch-proxy status --game-dir "$fixture" >"$dir/deploy-before.json"
jq -e '.status == "not_installed" and .launcher_changed == false and .game_process_touched == false' "$dir/deploy-before.json" >/dev/null
"$manager" launch-proxy install --game-dir "$fixture" \
  --proxy "$dir/steam_api64-eastward-preview.dll" \
  --dll "$dir/intimatr.dll" \
  --config "$(pwd)/config/Eastward.exe.toml" \
  --offline-single-player --game-closed >"$dir/deploy-installed.json"
jq -e '.status == "installed" and .original_preserved == true and .timer_hooks_activated == false' "$dir/deploy-installed.json" >/dev/null
cmp "$fixture/steam_api64.dll" "$dir/steam_api64-eastward-preview.dll"
cmp "$fixture/steam_api64_intimatr_original.dll" "$dir/steam_api64_intimatr_original.dll"
if "$manager" launch-proxy install --game-dir "$fixture" \
  --proxy "$dir/steam_api64-eastward-preview.dll" \
  --dll "$dir/intimatr.dll" \
  --config "$(pwd)/config/Eastward.exe.toml" \
  --offline-single-player --game-closed >"$dir/deploy-duplicate.stdout" 2>"$dir/deploy-duplicate.stderr"; then
  echo "ERROR: duplicate proxy installation was accepted" >&2
  exit 1
fi
"$manager" launch-proxy status --game-dir "$fixture" >"$dir/deploy-status.json"
jq -e '.status == "installed"' "$dir/deploy-status.json" >/dev/null
"$manager" launch-proxy remove --game-dir "$fixture" \
  --offline-single-player --game-closed >"$dir/deploy-removed.json"
jq -e '.status == "not_installed" and .launcher_changed == true' "$dir/deploy-removed.json" >/dev/null
cmp "$fixture/steam_api64.dll" "$dir/steam_api64_intimatr_original.dll"
test ! -e "$fixture/steam_api64_intimatr_original.dll"
test ! -e "$fixture/intimatr.dll"
test ! -e "$fixture/config/Eastward.exe.toml"
test ! -e "$fixture/intimatr-launch-deployment.json"
echo "Reversible native Eastward-shaped deployment verified without modifying a real game"
