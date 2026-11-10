#!/usr/bin/env bash
# Mock two native Bottles games; never load Wine or patch a real executable.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for cmd in jq sha256sum cmp realpath; do command -v "$cmd" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-batch-profiles-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
export MOCK_ROOT="$tmp/root" MOCK_PROGRAMS="$tmp/programs.json" MOCK_EVENTS="$tmp/events" MOCK_SOURCE="$tmp/speed.toml"
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
    if [[ -n "${MOCK_MUTATE_SOURCE_EXE:-}" &&
          "$*" == *"$MOCK_MUTATE_SOURCE_EXE"* ]]; then
      printf '# concurrent source policy change\n' >>"$MOCK_SOURCE"
    fi
    disabled=false
    if [[ -n "${MOCK_DISABLED_EXE:-}" && "$*" == *"$MOCK_DISABLED_EXE"* ]]; then
      disabled=true
    fi
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
    if [[ "$disabled" == true ]]; then
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
  profile-launch)
    if [[ -n "${MOCK_QA_FAIL_EXE:-}" && "$*" == *"$MOCK_QA_FAIL_EXE"* ]]; then
      echo "Simulated blocked native launch preflight" >&2
      exit 51
    fi
    requested=false
    if [[ " $* " == *" --experimental-four-timers "* ]]; then requested=true; fi
    jq -nc --argjson timers "$requested" '{
      operation:"read_only_registered_bottles_profile_launch_preflight",
      source_game_policy_verified:true,
      profile_file_matching_source_verified:true,
      bottles_environment_policy_preflight_passed:true,
      registered_program_environment_preflight_passed:true,
      mangohud_managed_config_checksum_verified:true,
      mangohud_managed_config_auto_selected:true,
      experimental_startup_timer_activation_requested:$timers,
      bottles_run_invoked:false,game_process_started_verified:false,
      bottle_configuration_modified:false,attachment_attempted:false
    }'
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

# A second profile's preview must not silently mix a concurrently edited
# shared speed policy with the first game's configuration.
export MOCK_MUTATE_SOURCE_EXE='C:\Games\Second\Second.exe'
if bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed \
  >"$tmp/source-drift.out" 2>"$tmp/source-drift.err"; then
  echo "Accepted source-policy drift during batch" >&2; exit 1
fi
grep -Fq 'Source speed policy changed during batch' "$tmp/source-drift.err"
test ! -e "$first/.intimatr-managed-launch"
test ! -e "$second/.intimatr-managed-launch"
unset MOCK_MUTATE_SOURCE_EXE
printf 'shared speed policy\n' >"$source"

bash "$script" prepare "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed >"$tmp/prepare.out"
test -f "$first/.intimatr-managed-launch/speed.toml"
test -f "$second/.intimatr-managed-launch/speed.toml"
jq -e '.renderer == "opengl"' "$first/.intimatr-managed-launch/launch.json" >/dev/null
jq -e '.renderer == "vulkan"' "$second/.intimatr-managed-launch/launch.json" >/dev/null
bash "$script" status "$manifest" >"$tmp/status.out"
test "$(grep -c 'owned_unchanged' "$tmp/status.out")" -eq 2

# Batch QA preflight must evaluate every reviewed game even when a later
# one is blocked, and must never launch Wine or modify either profile.
qa="scripts/bottles-qa-preflight.sh"
bash -n "$qa"
bash "$qa" "$manifest" --offline-single-player >"$tmp/qa-all.out"
jq -s -e --arg first "$first" --arg second "$second" '
  length == 3 and
  .[0].game_dir == $first and .[1].game_dir == $second and
  .[0].renderer == "opengl" and .[0].status == "ready_for_live_qa" and
  .[1].renderer == "vulkan" and .[1].status == "ready_for_live_qa" and
  .[2].operation == "live_qa_preflight_summary" and
  .[2].selected_games == 2 and .[2].ready_for_live_qa == 2 and
  .[2].blocked_games == 0 and .[2].game_started == false
' "$tmp/qa-all.out" >/dev/null
export MOCK_QA_FAIL_EXE='C:\Games\Second\Second.exe'
if bash "$qa" "$manifest" --offline-single-player \
  >"$tmp/qa-partial.out" 2>"$tmp/qa-partial.err"; then
  echo "Batch QA ignored a blocked registered second game" >&2
  exit 1
fi
jq -s -e '
  length == 3 and
  .[0].status == "ready_for_live_qa" and
  .[1].status == "blocked" and
  .[1].blocker == "launch_preflight_failed" and
  .[2].selected_games == 2 and
  .[2].ready_for_live_qa == 1 and .[2].blocked_games == 1
' "$tmp/qa-partial.out" >/dev/null
grep -Fq 'Simulated blocked native launch preflight' "$tmp/qa-partial.err"
unset MOCK_QA_FAIL_EXE
test -f "$first/.intimatr-managed-launch/speed.toml"
test -f "$second/.intimatr-managed-launch/speed.toml"

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
