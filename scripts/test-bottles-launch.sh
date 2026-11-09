#!/usr/bin/env bash
# Isolated exact registered-game launcher tests; never launch Wine or any game.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for tool in jq realpath; do command -v "$tool" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-launch-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
game="$tmp/root/Alpha/drive_c/Games/Demo"
mkdir -p "$tmp/bin" "$game"
printf 'fixture PE\n' >"$game/Demo.exe"
printf 'policy\n' >"$tmp/policy.toml"
printf 'delivered profile\n' >"$tmp/profile.toml"
export MOCK_ROOT="$tmp/root" MOCK_EVENTS="$tmp/events" MOCK_PROGRAMS="$tmp/programs.json"
cat >"$tmp/bin/bottles-cli" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "--json info bottles-path") echo "$MOCK_ROOT" ;;
  "--json programs -b Alpha") cat "$MOCK_PROGRAMS" ;;
  *) echo "Unexpected Bottles request: $*" >&2; exit 3 ;;
esac
EOF
cat >"$tmp/bin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$MOCK_EVENTS"
printf '{"operation":"fixture_profile_launch","bottles_run_invoked":false}\n'
EOF
cat >"$tmp/bin/x86_64-w64-mingw32-objdump" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == -p && "$2" == *.exe ]] || exit 3
case "$MOCK_IMPORTS" in
  opengl) echo '  DLL Name: opengl32.dll' ;;
  vulkan) echo '  DLL Name: vulkan-1.dll' ;;
  ambiguous) printf '%s\n' '  DLL Name: opengl32.dll' '  DLL Name: vulkan-1.dll' ;;
  none) echo '  DLL Name: kernel32.dll' ;;
  *) exit 3 ;;
esac
EOF
chmod 0755 "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
jq -n '[{name:"Demo Offline",path:"C:\\Games\\Demo\\Demo.exe"}]' >"$MOCK_PROGRAMS"
jq -n --arg dir "$game" '{
  version:1,games:[{
    game_dir:$dir,executable:"Demo.exe",bottle:"Alpha",
    registered_program:"Demo Offline",
    windows_executable:"C:\\Games\\Demo\\Demo.exe"
  }]
}' >"$tmp/selection.json"
manager="$tmp/bin/intimatr-bottles"
manifest="$tmp/selection.json"
source="$tmp/policy.toml"
profile="$tmp/profile.toml"
bash -n scripts/bottles-launch.sh

export MOCK_IMPORTS=opengl
bash scripts/bottles-launch.sh check "$manifest" "$manager" "$game" "$source" "$profile" \
  --offline-single-player --experimental-four-timers >"$tmp/gl.out"
grep -Fq -- '--mangohud-dlsym' "$MOCK_EVENTS"
grep -Fq -- '--experimental-four-timers' "$MOCK_EVENTS"
grep -Fq -- '--check' "$MOCK_EVENTS"
grep -Fq -- '--managed-mangohud' "$MOCK_EVENTS"

export MOCK_IMPORTS=vulkan
bash scripts/bottles-launch.sh check "$manifest" "$manager" "$game" "$source" "$profile" \
  --offline-single-player >"$tmp/vk.out"
tail -n 1 "$MOCK_EVENTS" | grep -Fq -- '--managed-mangohud'
if tail -n 1 "$MOCK_EVENTS" | grep -Fq -- '--mangohud-dlsym'; then
  echo 'Vulkan incorrectly enabled OpenGL preload' >&2
  exit 1
fi

export MOCK_IMPORTS=ambiguous
before="$(wc -l <"$MOCK_EVENTS")"
if bash scripts/bottles-launch.sh check "$manifest" "$manager" "$game" "$source" "$profile" \
  --offline-single-player >"$tmp/ambiguous.out" 2>"$tmp/ambiguous.err"; then
  echo 'Ambiguous renderer silently guessed a mode' >&2
  exit 1
fi
grep -Fq 'Renderer is ambiguous or dynamically loaded' "$tmp/ambiguous.err"
test "$before" -eq "$(wc -l <"$MOCK_EVENTS")"

# An explicit renderer is allowed even if PE imports are ambiguous.
bash scripts/bottles-launch.sh check "$manifest" "$manager" "$game" "$source" "$profile" \
  --renderer opengl --offline-single-player >"$tmp/explicit.out"
tail -n 1 "$MOCK_EVENTS" | grep -Fq -- '--mangohud-dlsym'

# Registration changes always require a new reviewed manifest.
jq -n '[{name:"Demo Offline",path:"C:\\Different\\Demo.exe"}]' >"$MOCK_PROGRAMS"
before="$(wc -l <"$MOCK_EVENTS")"
if bash scripts/bottles-launch.sh check "$manifest" "$manager" "$game" "$source" "$profile" \
  --renderer opengl --offline-single-player >"$tmp/changed.out" 2>"$tmp/changed.err"; then
  echo 'Changed native registration was accepted' >&2
  exit 1
fi
grep -Fq 'Registered Bottles program changed' "$tmp/changed.err"
test "$before" -eq "$(wc -l <"$MOCK_EVENTS")"

# A registration for another file in this bottle must not silently launch
# that other program even if the manifest and native registry agree on it.
jq '.games[0].windows_executable="C:\\Games\\Other\\Demo.exe"' "$manifest" >"$tmp/wrong-target.json"
jq -n '[{name:"Demo Offline",path:"C:\\Games\\Other\\Demo.exe"}]' >"$MOCK_PROGRAMS"
before="$(wc -l <"$MOCK_EVENTS")"
if bash scripts/bottles-launch.sh check "$tmp/wrong-target.json" "$manager" "$game" \
  "$source" "$profile" --renderer opengl --offline-single-player \
  >"$tmp/wrong-target.out" 2>"$tmp/wrong-target.err"; then
  echo 'Launch accepted another executable inside the selected bottle' >&2
  exit 1
fi
grep -Fq 'Registered executable path does not match selected game directory' "$tmp/wrong-target.err"
test "$before" -eq "$(wc -l <"$MOCK_EVENTS")"

# The exact file, policy and profile are read-only in every case.
test -f "$game/Demo.exe"
test ! -e "$game/MangoHud.conf"
test ! -e "$game/intimatr.dll"
test "$(cat "$source")" = policy
test "$(cat "$profile")" = 'delivered profile'
echo 'Bottles launcher fixture: renderer selection, explicit override, registry drift, and read-only preflight passed'
