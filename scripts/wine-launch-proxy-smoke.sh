#!/usr/bin/env bash
# Disposable Wine startup regression; does not inspect or modify real games.
set -euo pipefail
dir="$(pwd)/target/wine-launch-proxy"
mkdir -p "$dir/config"
for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-dlltool x86_64-w64-mingw32-objdump wine xvfb-run; do command -v "$tool" >/dev/null; done
cp target/x86_64-pc-windows-gnu/release/intimatr.dll "$dir/intimatr.dll"
cp config/steam_api64_launch_probe.exe.toml "$dir/config/"
x86_64-w64-mingw32-gcc -shared -o "$dir/steam_api64.dll" native/launch_proxy/steam_api64_original_fixture.c -Wl,--out-implib,"$dir/libsteam_fixture.a"
x86_64-w64-mingw32-gcc -o "$dir/steam_api64_launch_probe.exe" native/launch_proxy/steam_api64_launch_probe.c "$dir/libsteam_fixture.a"
mv "$dir/steam_api64.dll" "$dir/steam_api64_intimatr_original.dll"
INTIMATR_OBJDUMP=x86_64-w64-mingw32-objdump bash scripts/generate-steam-api64-forwarders.sh "$dir/steam_api64_intimatr_original.dll" "$dir/forwarders.def"
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
