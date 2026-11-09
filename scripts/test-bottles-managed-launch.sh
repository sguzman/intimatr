#!/usr/bin/env bash
# Isolated prepared-profile tests. No real Bottles, Wine or game is launched.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for tool in jq sha256sum cmp realpath; do command -v "$tool" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-managed-fixture.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/root/Alpha/drive_c/Games/Demo"
game="$tmp/root/Alpha/drive_c/Games/Demo"
printf 'mock Windows executable\n' >"$game/Demo.exe"
printf 'sample speed policy\n' >"$tmp/source.toml"
export MOCK_ROOT="$tmp/root" MOCK_PROGRAMS="$tmp/programs.json" MOCK_CALLS="$tmp/calls"
jq -n '[{name:"Demo Offline",path:"C:\\Games\\Demo\\Demo.exe"}]' >"$MOCK_PROGRAMS"
jq -n --arg game "$game" '{
  version:1,games:[{
    game_dir:$game,executable:"Demo.exe",bottle:"Alpha",
    registered_program:"Demo Offline",
    windows_executable:"C:\\Games\\Demo\\Demo.exe"
  }]
}' >"$tmp/selection.json"
cat >"$tmp/bin/bottles-cli" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "--json info bottles-path") echo "$MOCK_ROOT" ;;
  "--json programs -b Alpha") cat "$MOCK_PROGRAMS" ;;
  *) echo "Unexpected native Bottles request: $*" >&2; exit 3 ;;
esac
EOF
cat >"$tmp/bin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$MOCK_CALLS"
case "$1" in
  profile-check)
    printf '{"operation":"read_only_profile_resolution"}\n'
    ;;
  profile-export)
    [[ -z "${MOCK_EXPORT_FAIL:-}" ]] || exit 31
    output=""
    while (( $# )); do
      if [[ "$1" == --output ]]; then
        output="$2"
        break
      fi
      shift
    done
    [[ "$output" == /* && ! -e "$output" ]] || exit 3
    printf 'version = 1\n[speed]\nenabled = true\n' >"$output"
    jq -n --arg profile "$output" '{
      operation:"explicit_launch_profile_export",
      output_linux_file:$profile,resolved_speed_enabled:true
    }'
    ;;
  profile-launch)
    printf '{"operation":"fixture_profile_launch","bottles_run_invoked":false}\n'
    ;;
  *) echo "Unexpected Intimatr call: $*" >&2; exit 3 ;;
esac
EOF
chmod 0755 "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
manager="$tmp/bin/intimatr-bottles"
manifest="$tmp/selection.json"
source="$tmp/source.toml"
script="scripts/bottles-managed-launch.sh"
managed="$game/.intimatr-managed-launch"
bash -n "$script"

# A registered Windows path may vary by ASCII case from the Linux file.
# Still reject a genuinely different executable under the same prefix.
jq --arg w 'c:\games\demo\DEMO.exe' '.games[0].windows_executable=$w' \
  "$manifest" >"$tmp/case.json"
jq -n --arg w 'c:\games\demo\DEMO.exe' '[{name:"Demo Offline",path:$w}]' \
  >"$MOCK_PROGRAMS"
bash "$script" plan "$tmp/case.json" "$manager" "$game" "$source" \
  --renderer opengl >"$tmp/case.out"
test ! -e "$managed"
jq -n --arg w 'C:\Games\Other\Demo.exe' '[{name:"Demo Offline",path:$w}]' \
  >"$MOCK_PROGRAMS"
jq --arg w 'C:\Games\Other\Demo.exe' '.games[0].windows_executable=$w' \
  "$manifest" >"$tmp/other-game.json"
if bash "$script" plan "$tmp/other-game.json" "$manager" "$game" "$source" \
  --renderer opengl >"$tmp/other.out" 2>"$tmp/other.err"; then
  echo 'Different registered Windows game accepted as selected executable' >&2
  exit 1
fi
grep -Fq 'not the selected executable' "$tmp/other.err"
jq -n '[{name:"Demo Offline",path:"C:\\Games\\Demo\\Demo.exe"}]' >"$MOCK_PROGRAMS"

bash "$script" plan "$manifest" "$manager" "$game" "$source" \
  --renderer opengl >"$tmp/plan.out"
test ! -e "$managed"
grep -Fq 'managed_launch_plan' "$tmp/plan.out"

# Explicit preparation creates a private, checksum-owned profile without
# writing the registered game executable or installing another DLL.
game_before="$(sha256sum <"$game/Demo.exe" | cut -d' ' -f1)"
bash "$script" prepare "$manifest" "$manager" "$game" "$source" \
  --renderer opengl --offline-single-player --games-closed >"$tmp/prepare.out"
test "$(stat -c %a "$managed")" = 700
test -f "$managed/speed.toml"
test -f "$managed/launch.json"
test -f "$managed/snapshots.sha256"
test "$game_before" = "$(sha256sum <"$game/Demo.exe" | cut -d' ' -f1)"
test ! -e "$game/intimatr.dll"
test ! -e "$game/MangoHud.conf"
bash "$script" status "$game" >"$tmp/status.out"
grep -Fq 'owned_unchanged' "$tmp/status.out"

# Same selected game, source bytes and renderer: true no-op.
snapshot="$(sha256sum <"$managed/speed.toml" | cut -d' ' -f1)"
bash "$script" prepare "$manifest" "$manager" "$game" "$source" \
  --renderer opengl --offline-single-player --games-closed >"$tmp/idempotent.out"
grep -Fq 'already prepared unchanged' "$tmp/idempotent.out"
test "$snapshot" = "$(sha256sum <"$managed/speed.toml" | cut -d' ' -f1)"
bash "$script" plan "$manifest" "$manager" "$game" "$source" \
  --renderer opengl >"$tmp/plan-unchanged.out"
grep -Fq 'zero writes' "$tmp/plan-unchanged.out"

# One short check or launch retains the exact registered identity and renderer.
bash "$script" check "$game" --offline-single-player \
  --experimental-four-timers >"$tmp/check.out"
tail -n 1 "$MOCK_CALLS" | grep -Fq -- '--mangohud-dlsym'
tail -n 1 "$MOCK_CALLS" | grep -Fq -- '--experimental-four-timers'
tail -n 1 "$MOCK_CALLS" | grep -Fq -- '--check'
bash "$script" launch "$game" --offline-single-player >"$tmp/launch.out"
tail -n 1 "$MOCK_CALLS" | grep -Fq -- '--launch'
if tail -n 1 "$MOCK_CALLS" | grep -Fq -- '--experimental-four-timers'; then
  echo 'Managed launch armed experimental hooks without opt-in' >&2
  exit 1
fi

# A real app-launcher entry is optional and explicitly selected.
# No game launches during shortcut plan/install; only Intimatr --check runs.
export XDG_DATA_HOME="$tmp/xdg data"
shortcut_script="scripts/bottles-shortcut.sh"
bash -n "$shortcut_script"
bash "$shortcut_script" plan "$game" --experimental-four-timers >"$tmp/shortcut-plan.out"
grep -Fq 'shortcut_plan' "$tmp/shortcut-plan.out"
test ! -d "$XDG_DATA_HOME/applications"
if bash "$shortcut_script" install "$game" >"$tmp/no-consent-desktop.out" 2>"$tmp/no-consent-desktop.err"; then
  echo "Shortcut creation accepted missing offline-single-player consent" >&2
  exit 1
fi
bash "$shortcut_script" install "$game" --offline-single-player \
  --experimental-four-timers >"$tmp/shortcut-install.out"
shortcuts=("$XDG_DATA_HOME/applications"/intimatr-bottles-*.desktop)
test "${#shortcuts[@]}" -eq 1
desktop="${shortcuts[0]}"
test -f "$desktop"
grep -Fq 'Categories=Game;' "$desktop"
grep -Fq -- '--experimental-four-timers' "$desktop"
grep -Fq -- '--offline-single-player' "$desktop"
grep -Fq -- '"/Games/Demo"' "$desktop" && {
  echo 'Desktop entry unexpectedly lost full executable directory' >&2
  exit 1
}
test "$(grep -c '^Exec=' "$desktop")" -eq 1
bash "$shortcut_script" status "$game" >"$tmp/shortcut-status.out"
grep -Fq 'owned_unchanged' "$tmp/shortcut-status.out"
desktop_sha="$(sha256sum <"$desktop" | cut -d' ' -f1)"
bash "$shortcut_script" install "$game" --offline-single-player \
  --experimental-four-timers >"$tmp/shortcut-repeat.out"
grep -Fq 'already installed unchanged' "$tmp/shortcut-repeat.out"
test "$desktop_sha" = "$(sha256sum <"$desktop" | cut -d' ' -f1)"
printf '# local modification\n' >>"$desktop"
bash "$shortcut_script" status "$game" >"$tmp/shortcut-modified.out"
grep -Fq 'modified_or_unmanaged' "$tmp/shortcut-modified.out"
if bash "$shortcut_script" remove "$game" >"$tmp/shortcut-refuse.out" 2>"$tmp/shortcut-refuse.err"; then
  echo "Shortcut remove deleted an edited desktop entry" >&2
  exit 1
fi
grep -Fq 'refusing removal' "$tmp/shortcut-refuse.err"
sed -i '$d' "$desktop"
test "$desktop_sha" = "$(sha256sum <"$desktop" | cut -d' ' -f1)"
bash "$shortcut_script" remove "$game" >"$tmp/shortcut-remove.out"
test ! -e "$desktop"
test -f "$game/Demo.exe"
test -d "$managed"

# A changed source policy must not launch a stale profile, but ownership-
# checked removal must still work without the original source file.
printf 'user changed the speed settings\n' >>"$source"
before="$(wc -l <"$MOCK_CALLS")"
if bash "$script" check "$game" --offline-single-player \
  >"$tmp/source-drift.out" 2>"$tmp/source-drift.err"; then
  echo 'Managed launch ignored changed source policy' >&2
  exit 1
fi
grep -Fq 'Source speed policy changed' "$tmp/source-drift.err"
test "$before" -eq "$(wc -l <"$MOCK_CALLS")"

# Refuse extra user-owned files or modified managed profile before removal.
touch "$managed/custom.txt"
if bash "$script" remove "$game" --games-closed >"$tmp/extra.out" 2>"$tmp/extra.err"; then
  echo 'Managed remove deleted extra user file' >&2
  exit 1
fi
test -f "$managed/custom.txt"
rm -- "$managed/custom.txt"
printf '# local edit\n' >>"$managed/speed.toml"
if bash "$script" remove "$game" --games-closed >"$tmp/tamper.out" 2>"$tmp/tamper.err"; then
  echo 'Managed remove deleted edited profile' >&2
  exit 1
fi
grep -Fq 'checksum mismatch' "$tmp/tamper.err"
test -f "$managed/speed.toml"
# The fixture can put its own original bytes back; no user state is altered.
printf 'version = 1\n[speed]\nenabled = true\n' >"$managed/speed.toml"
bash "$script" remove "$game" --games-closed >"$tmp/remove.out"
test ! -e "$managed"
test "$game_before" = "$(sha256sum <"$game/Demo.exe" | cut -d' ' -f1)"

# A pre-existing unmanaged directory must never be overwritten.
mkdir -- "$managed"
printf 'user managed\n' >"$managed/custom.txt"
if bash "$script" prepare "$manifest" "$manager" "$game" "$source" \
  --renderer vulkan --offline-single-player --games-closed \
  >"$tmp/unmanaged.out" 2>"$tmp/unmanaged.err"; then
  echo 'Managed preparation overwrote unmanaged files' >&2
  exit 1
fi
test "$(cat "$managed/custom.txt")" = "user managed"
echo 'Managed launch fixture: plan, owned prepare, no-op, short launch, drift, removal, and conflict refusal passed'
