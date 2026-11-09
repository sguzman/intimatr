#!/usr/bin/env bash
# End-to-end fixture for one reviewed plan/apply; no actual Wine or user games.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for cmd in jq realpath sha256sum cmp; do command -v "$cmd" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-full-setup-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
game="$tmp/root/Alpha/drive_c/Games/Demo"
mkdir -p "$tmp/bin" "$game"
printf 'mock game executable\n' >"$game/Demo.exe"
printf 'original managed DLL\n' >"$game/intimatr.dll"
printf '{}\n' >"$game/intimatr-launch-deployment.json"
printf 'source speed policy\n' >"$tmp/source.toml"
export MOCK_ROOT="$tmp/root" MOCK_EVENTS="$tmp/events" MOCK_PROGRAMS="$tmp/programs.json"
jq -n '[{name:"Demo Offline",path:"C:\\Games\\Demo\\Demo.exe"}]' >"$MOCK_PROGRAMS"
jq -n --arg game "$game" '{version:1,games:[{
  game_dir:$game,executable:"Demo.exe",bottle:"Alpha",
  registered_program:"Demo Offline",
  windows_executable:"C:\\Games\\Demo\\Demo.exe",renderer:"opengl"
}]}' >"$tmp/reviewed.json"
cat >"$tmp/bin/bottles-cli" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "--json info bottles-path") echo "$MOCK_ROOT" ;;
  "--json list bottles") printf '{"Alpha":{}}\n' ;;
  "--json programs -b Alpha") cat "$MOCK_PROGRAMS" ;;
  *) echo "Unexpected native Bottles CLI: $*" >&2; exit 3 ;;
esac
EOF
cat >"$tmp/bin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$MOCK_EVENTS"
case "$1" in
  bottle-env)
    if [[ "$2" == status ]]; then
      printf '{"operation":"status","bottle":"Alpha","ownership":"unmanaged","allowlist_entries_to_add":["MANGOHUD"]}\n'
    else
      printf '{"operation":"fixture_bottle_env","files_modified":false}\n'
    fi
    ;;
  launch-proxy)
    [[ "$2" == status && "$3" == --game-dir ]] || exit 3
    printf '{"status":"installed","original_preserved":true}\n'
    ;;
  profile-check)
    printf '{"operation":"read_only_profile_resolution"}\n'
    ;;
  profile-export)
    output=""
    while (( $# )); do
      if [[ "$1" == --output ]]; then output="$2"; break; fi
      shift
    done
    [[ "$output" == /* && ! -e "$output" ]] || exit 3
    printf 'version = 1\n[speed]\nenabled = true\n' >"$output"
    jq -n --arg p "$output" '{
      operation:"explicit_launch_profile_export",output_linux_file:$p,
      resolved_speed_enabled:true
    }'
    ;;
  profile-launch)
    echo "Game launch was forbidden by this fixture" >&2; exit 99 ;;
  *) echo "Unexpected Intimatr action: $*" >&2; exit 3 ;;
esac
EOF
chmod 0755 "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
script="scripts/bottles-full-setup.sh"
manager="$tmp/bin/intimatr-bottles"
manifest="$tmp/reviewed.json"
source="$tmp/source.toml"
before="$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"
bash -n "$script"

bash "$script" plan "$manifest" "$manager" "$source" \
  >"$tmp/plan.out"
grep -Fq 'Combined plan complete' "$tmp/plan.out"
test ! -e "$game/MangoHud.conf"
test ! -e "$game/.intimatr-managed-launch"

if bash "$script" apply "$manifest" "$manager" "$source" \
  >"$tmp/no-consent.out" 2>"$tmp/no-consent.err"; then
  echo "Combined setup accepted missing offline/closed acknowledgements" >&2
  exit 1
fi
test ! -e "$game/MangoHud.conf"

# Full combined apply installs only the missing owned HUD and game profile.
bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/applied.out"
grep -Fq 'Combined setup complete' "$tmp/applied.out"
grep -Fq 'Recovery journal:' "$tmp/applied.out"
test -f "$game/MangoHud.conf"
test -f "$game/intimatr-mangohud.sha256"
test -f "$game/.intimatr-managed-launch/speed.toml"
test -f "$game/.intimatr-managed-launch/launch.json"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"
test -f "$game/intimatr-launch-deployment.json"
test ! -e "$game/.intimatr-managed-launch/extra"
if grep -Fq 'profile-launch' "$MOCK_EVENTS"; then
  echo "Combined setup attempted to launch a game" >&2; exit 1
fi
journals=("$tmp/root"/.intimatr-setup.*)
test "${#journals[@]}" -eq 1
test "$(cat "${journals[0]}/stage")" = complete

bash "$script" status "$manifest" "$manager" >"$tmp/status.out"
grep -Fq 'owned_unchanged' "$tmp/status.out"
grep -Fq 'HUD owned (unchanged)' "$tmp/status.out"

# Idempotent reapplication does not change either file, game binary or DLL.
hud_sha="$(sha256sum <"$game/MangoHud.conf" | cut -d' ' -f1)"
profile_sha="$(sha256sum <"$game/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/reapply.out"
grep -Fq 'already prepared unchanged' "$tmp/reapply.out"
test "$hud_sha" = "$(sha256sum <"$game/MangoHud.conf" | cut -d' ' -f1)"
test "$profile_sha" = "$(sha256sum <"$game/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"

# A newly changed source cannot be silently adopted on an existing profile.
printf '# edited by owner\n' >>"$source"
if bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/policy-change.out" 2>"$tmp/policy-change.err"; then
  echo "Combined setup accepted a changed source profile" >&2; exit 1
fi
grep -Fq 'differs from reviewed request' "$tmp/policy-change.err"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"
test "$hud_sha" = "$(sha256sum <"$game/MangoHud.conf" | cut -d' ' -f1)"
echo 'Full setup fixture: one plan/apply/status, idempotence, profile/HUD integrity, and no game launch passed'
