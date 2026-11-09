#!/usr/bin/env bash
# Mock two native Bottles games; never load Wine or patch a real executable.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for cmd in jq sha256sum cmp realpath; do command -v "$cmd" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-batch-profiles-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
export MOCK_ROOT="$tmp/root" MOCK_PROGRAMS="$tmp/programs.json" MOCK_EVENTS="$tmp/events"
mkdir -p "$tmp/bin" "$MOCK_ROOT/Alpha/drive_c/Games/First" \
  "$MOCK_ROOT/Alpha/drive_c/Games/Second"
first="$MOCK_ROOT/Alpha/drive_c/Games/First"
second="$MOCK_ROOT/Alpha/drive_c/Games/Second"
printf 'first game binary\n' >"$first/First.exe"
printf 'second game binary\n' >"$second/Second.exe"
printf 'shared speed policy\n' >"$tmp/speed.toml"
jq -n '[
  {name:"First Offline",path:"C:\\Games\\First\\First.exe"},
  {name:"Second Offline",path:"C:\\Games\\Second\\Second.exe"}
]' >"$MOCK_PROGRAMS"
jq -n --arg first "$first" --arg second "$second" '{
  version:1,games:[
    {game_dir:$first,executable:"First.exe",bottle:"Alpha",
     registered_program:"First Offline",windows_executable:"C:\\Games\\First\\First.exe",
     renderer:"opengl"},
    {game_dir:$second,executable:"Second.exe",bottle:"Alpha",
     registered_program:"Second Offline",windows_executable:"C:\\Games\\Second\\Second.exe",
     renderer:"vulkan"}
  ]}' >"$tmp/selected.json"
cat >"$tmp/bin/bottles-cli" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "--json info bottles-path") echo "$MOCK_ROOT" ;;
  "--json programs -b Alpha") cat "$MOCK_PROGRAMS" ;;
  *) echo "Unexpected Bottles invocation: $*" >&2; exit 3 ;;
esac
EOF
cat >"$tmp/bin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$MOCK_EVENTS"
case "$1" in
  profile-check) printf '{"operation":"read_only_profile_resolution"}\n' ;;
  profile-export)
    if [[ -n "${MOCK_FAIL_EXE:-}" && "$*" == *"$MOCK_FAIL_EXE"* &&
          "$*" == *"/.intimatr-managed-launch/"* ]]; then
      exit 44
    fi
    output=""
    while (( $# )); do
      if [[ "$1" == --output ]]; then output="$2"; break; fi
      shift
    done
    [[ "$output" == /* && ! -e "$output" ]] || exit 3
    if [[ -n "${MOCK_DISABLED_EXE:-}" && "$*" == *"$MOCK_DISABLED_EXE"* ]]; then
      printf 'version = 1\n[speed]\nenabled = false\n' >"$output"
      jq -n --arg p "$output" '{
        operation:"explicit_launch_profile_export",output_linux_file:$p,
        resolved_speed_enabled:false
      }'
    else
      printf 'version = 1\n[speed]\nenabled = true\n' >"$output"
      jq -n --arg p "$output" '{
        operation:"explicit_launch_profile_export",output_linux_file:$p,
        resolved_speed_enabled:true
      }'
    fi
    ;;
  *) echo "Unexpected native Intimatr command: $*" >&2; exit 3 ;;
esac
EOF
chmod 0755 "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
script="scripts/bottles-profiles-batch.sh"
single="scripts/bottles-managed-launch.sh"
manifest="$tmp/selected.json"
manager="$tmp/bin/intimatr-bottles"
source="$tmp/speed.toml"
bash -n "$script"

bash "$script" plan "$manifest" "$manager" "$source" >"$tmp/plan.out"
test ! -e "$first/.intimatr-managed-launch"
test ! -e "$second/.intimatr-managed-launch"
test "$(grep -c 'managed_launch_plan' "$tmp/plan.out")" -eq 2

# No implicit per-game renderer and no offline consent.
jq 'del(.games[1].renderer)' "$manifest" >"$tmp/missing-renderer.json"
if bash "$script" plan "$tmp/missing-renderer.json" "$manager" "$source" \
  >"$tmp/missing.out" 2>"$tmp/missing.err"; then
  echo "Accepted a candidate missing reviewed renderer" >&2; exit 1
fi
grep -Fq 'missing explicit per-game renderer' "$tmp/missing.err"
if bash "$script" prepare "$manifest" "$manager" "$source" \
  >"$tmp/unapproved.out" 2>"$tmp/unapproved.err"; then
  echo "Accepted bulk install without offline and closed-game acknowledgements" >&2
  exit 1
fi
test ! -e "$first/.intimatr-managed-launch"

# A disabled late-game profile must fail the full preview before the
# earlier game's managed directory is created at all.
export MOCK_DISABLED_EXE='C:\Games\Second\Second.exe'
if bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed \
  >"$tmp/disabled.out" 2>"$tmp/disabled.err"; then
  echo "Accepted a disabled late-game Windows profile" >&2; exit 1
fi
grep -Fq 'Preview profile disabled or invalid' "$tmp/disabled.err"
test ! -e "$first/.intimatr-managed-launch"
test ! -e "$second/.intimatr-managed-launch"
unset MOCK_DISABLED_EXE

bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed >"$tmp/prepare.out"
test -f "$first/.intimatr-managed-launch/speed.toml"
test -f "$second/.intimatr-managed-launch/speed.toml"
jq -e '.renderer == "opengl"' "$first/.intimatr-managed-launch/launch.json" >/dev/null
jq -e '.renderer == "vulkan"' "$second/.intimatr-managed-launch/launch.json" >/dev/null
bash "$script" status "$manifest" >"$tmp/status.out"
test "$(grep -c 'owned_unchanged' "$tmp/status.out")" -eq 2

# Repeated preparations do not write either profile.
first_sha="$(sha256sum <"$first/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed >"$tmp/repeated.out"
test "$(grep -c 'already prepared unchanged' "$tmp/repeated.out")" -eq 2
test "$first_sha" = "$(sha256sum <"$first/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"

# One modified profile must block the whole removal BEFORE either is touched.
printf '# owner change\n' >>"$second/.intimatr-managed-launch/speed.toml"
if bash "$script" remove "$manifest" --games-closed \
  >"$tmp/tamper.out" 2>"$tmp/tamper.err"; then
  echo "Batch removed an unchanged neighbor despite a modified profile" >&2
  exit 1
fi
test -f "$first/.intimatr-managed-launch/speed.toml"
grep -Fq 'checksum mismatch' "$tmp/tamper.err"
printf 'version = 1\n[speed]\nenabled = true\n' >"$second/.intimatr-managed-launch/speed.toml"
bash "$script" remove "$manifest" --games-closed >"$tmp/removed.out"
test ! -e "$first/.intimatr-managed-launch"
test ! -e "$second/.intimatr-managed-launch"

# A failed second export after successful first preparation compensates by
# removing ONLY the newly completed first profile. Incomplete current files
# remain untouched and clearly reported for manual inspection.
export MOCK_FAIL_EXE='C:\Games\Second\Second.exe'
if bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed >"$tmp/failed.out" 2>"$tmp/failed.err"; then
  echo "Second export failure was ignored" >&2; exit 1
fi
grep -Fq "Restored newly prepared profile: $first" "$tmp/failed.err"
grep -Fq 'INCOMPLETE:' "$tmp/failed.err"
test ! -e "$first/.intimatr-managed-launch"
test -d "$second/.intimatr-managed-launch"
test -f "$first/First.exe"
test -f "$second/Second.exe"
echo 'Bulk prepared profile fixture: multi-game plan, renderer, idempotence, conflict and failure compensation passed'
