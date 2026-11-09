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
