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
lock="$tmp/root/.intimatr-full-operation.lock"
test ! -e "$lock"

# A user-owned/unsafe file or symlink at the reserved lock path is not
# clobbered, and all selected game files stay untouched.
ln -s -- "$source" "$lock"
if bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/lock-symlink.out" 2>"$tmp/lock-symlink.err"; then
  echo "Combined setup followed a foreign lock symlink" >&2; exit 1
fi
test -L "$lock"
test ! -e "$game/MangoHud.conf"
rm -- "$lock"
test ! -e "$game/MangoHud.conf"
test ! -e "$game/.intimatr-managed-launch"

if bash "$script" apply "$manifest" "$manager" "$source" \
  >"$tmp/no-consent.out" 2>"$tmp/no-consent.err"; then
  echo "Combined setup accepted missing offline/closed acknowledgements" >&2
  exit 1
fi
test ! -e "$game/MangoHud.conf"

# A prior interrupted profile stage must block a new combined apply
# even when its base setup stage had already reached complete.
unfinished="$(mktemp -d "$tmp/root/.intimatr-setup.XXXXXXXX")"
printf 'profiles-started\n' >"$unfinished/full-profile-stage"
before_count="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS" || true)"
if bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/prior-incomplete.out" 2>"$tmp/prior-incomplete.err"; then
  echo "Combined setup ignored unresolved earlier profile journal" >&2
  exit 1
fi
grep -Fq 'Unfinished combined profile stage (profiles-started)' "$tmp/prior-incomplete.err"
test "$before_count" -eq "$(grep -c '^bottle-env apply ' "$MOCK_EVENTS" || true)"
test ! -e "$game/MangoHud.conf"
rm -- "$unfinished/full-profile-stage"
rmdir -- "$unfinished"

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
test -f "${journals[0]}/full-profiles-before.json"
test -f "${journals[0]}/full-profiles-before.sha256"
test "$(cat "${journals[0]}/full-profile-stage")" = profiles-complete
test -f "${journals[0]}/full-profiles-after.json"
test -f "${journals[0]}/full-profiles-after.sha256"
jq -e --arg dir "$game" '
  .version == 1 and (.games | length == 1) and
  .games[0].game_dir == $dir and
  (.games[0].snapshot_sha256 | test("^[0-9a-f]{64}$")) and
  (.games[0].directory_identity | test("^[0-9]+:[0-9]+$"))
' "${journals[0]}/full-profiles-after.json" >/dev/null
test -f "$lock"
test ! -L "$lock"
test "$(stat -c %a -- "$lock")" = 600
jq -e --arg dir "$game" '
  .version == 1 and (.games | length == 1) and
  .games[0].game_dir == $dir and .games[0].preexisting == false
' "${journals[0]}/full-profiles-before.json" >/dev/null
bash scripts/bottles-full-recover.sh status "${journals[0]}" "$manager" >"$tmp/recovery-status.out"
grep -Fq 'Eligible new profiles: 1; pre-existing preserved: 0; profile blockers: 0; base blockers: 0.' "$tmp/recovery-status.out"

bash "$script" status "$manifest" "$manager" >"$tmp/status.out"
grep -Fq 'owned_unchanged' "$tmp/status.out"
grep -Fq 'HUD owned (unchanged)' "$tmp/status.out"

# A valid, byte-identical managed profile installed *later* is not
# owned by the earlier full attempt. Directory identity distinguishes
# replacement even when all managed checksums are exactly identical.
original_managed="$game/.intimatr-managed-launch"
mv -- "$original_managed" "$tmp/original-owned-profile"
mkdir -- "$original_managed"
cp -a -- "$tmp/original-owned-profile/." "$original_managed/"
bash scripts/bottles-managed-launch.sh status "$game" >"$tmp/replacement-owned.out"
grep -Fq 'owned_unchanged' "$tmp/replacement-owned.out"
bash scripts/bottles-full-recover.sh status "${journals[0]}" "$manager" \
  >"$tmp/replacement-status.out" 2>"$tmp/replacement-status.err"
grep -Fq 'managed profile was replaced after the completed setup' "$tmp/replacement-status.err"
grep -Fq 'profile blockers: 1' "$tmp/replacement-status.out"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/replacement-rollback.out" 2>"$tmp/replacement-rollback.err"; then
  echo 'Recovery deleted a byte-identical profile created after setup' >&2
  exit 1
fi
test -f "$original_managed/speed.toml"
rm -- "$original_managed/launch.json" "$original_managed/speed.toml" \
  "$original_managed/snapshots.sha256"
rmdir -- "$original_managed"
mv -- "$tmp/original-owned-profile" "$original_managed"

# An edited completed-profile inventory must block recovery before any
# profile files are removed, even if its JSON remains syntactically valid.
cp -- "${journals[0]}/full-profiles-after.json" "$tmp/intact-profile-after.json"
printf '\n' >>"${journals[0]}/full-profiles-after.json"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/edited-after.out" 2>"$tmp/edited-after.err"; then
  echo 'Combined recovery trusted a modified completion inventory' >&2
  exit 1
fi
grep -Fq 'Completed profile identity snapshot checksum mismatch' "$tmp/edited-after.err"
cp -- "$tmp/intact-profile-after.json" "${journals[0]}/full-profiles-after.json"
test -f "$original_managed/speed.toml"

# Lock contention must reject overlapping combined operations before
# modifying any selected profile, HUD or environment.
exec 7<"$lock"
flock -n 7
if bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/contended-apply.out" 2>"$tmp/contended-apply.err"; then
  echo "Combined apply bypassed active full-operation lock" >&2; exit 1
fi
grep -Fq 'Another Intimatr combined setup or recovery is active' "$tmp/contended-apply.err"
test -f "$game/.intimatr-managed-launch/speed.toml"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/contended-rollback.out" 2>"$tmp/contended-rollback.err"; then
  echo "Combined rollback bypassed active full-operation lock" >&2; exit 1
fi
grep -Fq 'Another Intimatr combined setup or recovery is active' "$tmp/contended-rollback.err"
test "$(cat "${journals[0]}/full-profile-stage")" = profiles-complete
flock -u 7
exec 7<&-

# Idempotent reapplication does not change either file, game binary or DLL.
hud_sha="$(sha256sum <"$game/MangoHud.conf" | cut -d' ' -f1)"
profile_sha="$(sha256sum <"$game/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/reapply.out"
grep -Fq 'already prepared unchanged' "$tmp/reapply.out"
test "$hud_sha" = "$(sha256sum <"$game/MangoHud.conf" | cut -d' ' -f1)"
test "$profile_sha" = "$(sha256sum <"$game/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"

# The second completed journal records the already-managed profile as
# pre-existing. Recovering that attempt must never delete it, the original
# DLL, or the HUD first installed by the earlier attempt.
all_journals=("$tmp/root"/.intimatr-setup.*)
test "${#all_journals[@]}" -eq 2
second_journal=""
for candidate in "${all_journals[@]}"; do
  [[ "$candidate" == "${journals[0]}" ]] || second_journal="$candidate"
done
[[ -n "$second_journal" ]]
jq -e '.games[0].preexisting == true' "$second_journal/full-profiles-before.json" >/dev/null
bash scripts/bottles-full-recover.sh rollback "$second_journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/previous-profile.out"
grep -Fq 'pre-existing profiles' "$tmp/previous-profile.out"
test "$(cat "$second_journal/full-profile-stage")" = reverted
test -f "$game/.intimatr-managed-launch/speed.toml"
test -f "$game/MangoHud.conf"
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
# Recovery refuses to infer ownership when profile preparation did not
# reach a completed stage, or when the prepared profile was edited.
printf 'profiles-started\n' >"${journals[0]}/full-profile-stage"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/started-recovery.out" 2>"$tmp/started-recovery.err"; then
  echo 'Combined recovery assumed an incomplete profile stage completed' >&2
  exit 1
fi
grep -Fq 'interrupted profile stage' "$tmp/started-recovery.err"
test -f "$game/.intimatr-managed-launch/speed.toml"
printf 'profiles-complete\n' >"${journals[0]}/full-profile-stage"
printf '# owner changed profile\n' >>"$game/.intimatr-managed-launch/speed.toml"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/tamper-recovery.out" 2>"$tmp/tamper-recovery.err"; then
  echo 'Combined recovery removed a user-modified profile' >&2
  exit 1
fi
grep -Fq 'ownership conflicts prevent combined rollback' "$tmp/tamper-recovery.err"
test -f "$game/MangoHud.conf"
test -f "$game/.intimatr-managed-launch/speed.toml"
sed -i '$d' "$game/.intimatr-managed-launch/speed.toml"
test "$profile_sha" = "$(sha256sum <"$game/.intimatr-managed-launch/speed.toml" | cut -d' ' -f1)"
# Model a killed rollback AFTER it deleted the newly managed profile but
# BEFORE it reached base HUD/DLL/environment recovery. A retry must skip the
# already removed profile and finish only the remaining owned base work.
bash scripts/bottles-managed-launch.sh remove "$game" --games-closed \
  >"$tmp/partial-profile-removal.out"
test ! -e "$game/.intimatr-managed-launch"
printf 'rollback-started\n' >"${journals[0]}/full-profile-stage"
bash scripts/bottles-full-recover.sh status "${journals[0]}" "$manager" \
  >"$tmp/interrupted-rollback-status.out"
grep -Fq 'Eligible new profiles: 0; pre-existing preserved: 0; profile blockers: 0; base blockers: 0.' \
  "$tmp/interrupted-rollback-status.out"
bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/full-rollback.out"
grep -Fq 'Combined recovery complete' "$tmp/full-rollback.out"
# A retry after the base stage was also completed must be an idempotent
# no-op; it must never revive an already deleted profile.
printf 'rollback-started\n' >"${journals[0]}/full-profile-stage"
bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/retry-reverted.out"
grep -Fq 'Combined recovery complete' "$tmp/retry-reverted.out"
test ! -e "$game/.intimatr-managed-launch"
bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/again-reverted.out"
grep -Fq 'already reverted' "$tmp/again-reverted.out"
# Even a marker forged to "reverted" must not conceal an active base stage.
printf 'complete\n' >"${journals[0]}/stage"
if bash scripts/bottles-full-recover.sh rollback "${journals[0]}" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/false-reverted.out" 2>"$tmp/false-reverted.err"; then
  echo "Combined rollback accepted reverted profile marker with active base stage" >&2
  exit 1
fi
grep -Fq 'combined journal says reverted but base is complete' "$tmp/false-reverted.err"
printf 'reverted\n' >"${journals[0]}/stage"
test "$(cat "${journals[0]}/full-profile-stage")" = reverted
test "$(cat "${journals[0]}/stage")" = reverted
test ! -e "$game/.intimatr-managed-launch"
test ! -e "$game/MangoHud.conf"
test ! -e "$game/intimatr-mangohud.sha256"
test -f "$game/Demo.exe"
test -f "$game/intimatr-launch-deployment.json"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"
# The base installer can now durably capture the combined profile
# before-state BEFORE any base mutation. Simulate a crash between the base
# command finishing and the full coordinator resuming: there must be a
# "profiles-pending" journal, and no automatic profile removal permission.
jq -n --arg game "$game" '{
  version:1,games:[{game_dir:$game,preexisting:false}]
}' >"$tmp/before-base.json"
bash scripts/bottles-setup.sh apply "$manifest" "$manager" \
  --full-profiles-before "$tmp/before-base.json" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/base-only.out"
new_journal="$(sed -n 's/^Recovery journal: //p' "$tmp/base-only.out" | tail -n 1)"
test -d "$new_journal"
test "$(cat "$new_journal/stage")" = complete
test "$(cat "$new_journal/full-profile-stage")" = profiles-pending
cmp -s -- "$tmp/before-base.json" "$new_journal/full-profiles-before.json"
(
  cd -- "$new_journal"
  sha256sum -- full-profiles-before.json |
    cmp -s - full-profiles-before.sha256
)
test -f "$game/MangoHud.conf"
test ! -e "$game/.intimatr-managed-launch"
bash scripts/bottles-full-recover.sh status "$new_journal" "$manager" \
  >"$tmp/pending-status.out"
grep -Fq 'Profile stage: profiles-pending' "$tmp/pending-status.out"
grep -Fq 'Eligible new profiles: 0; pre-existing preserved: 0; profile blockers: 0; base blockers: 0.' \
  "$tmp/pending-status.out"

# A later unrelated managed directory cannot be attributed to the
# not-yet-started profile stage, even if it appears under the selected game.
mkdir -- "$game/.intimatr-managed-launch"
printf 'foreign user file\n' >"$game/.intimatr-managed-launch/custom.txt"
if bash scripts/bottles-full-recover.sh rollback "$new_journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/pending-foreign.out" 2>"$tmp/pending-foreign.err"; then
  echo 'Pending-stage recovery erased an unowned profile directory' >&2
  exit 1
fi
grep -Fq 'ownership conflicts prevent combined rollback' "$tmp/pending-foreign.err"
test -f "$game/.intimatr-managed-launch/custom.txt"
test -f "$game/MangoHud.conf"
rmdir -- "$game/.intimatr-managed-launch" 2>/dev/null && {
  echo 'Fixture unexpectedly removed a nonempty foreign directory' >&2
  exit 1
}
rm -- "$game/.intimatr-managed-launch/custom.txt"
rmdir -- "$game/.intimatr-managed-launch"

bash scripts/bottles-full-recover.sh rollback "$new_journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/pending-rollback.out"
grep -Fq 'Combined pending-stage recovery complete; no profiles removed.' \
  "$tmp/pending-rollback.out"
test "$(cat "$new_journal/full-profile-stage")" = profiles-pending-reverted
test "$(cat "$new_journal/stage")" = reverted
test ! -e "$game/.intimatr-managed-launch"
test ! -e "$game/MangoHud.conf"
test -f "$game/intimatr.dll"
test "$before" = "$(sha256sum <"$game/intimatr.dll" | cut -d' ' -f1)"
bash scripts/bottles-full-recover.sh rollback "$new_journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/pending-repeat.out"
grep -Fq 'Combined journal already reverted' "$tmp/pending-repeat.out"

# Reverted pending attempts must not block future combined setup.
bash "$script" apply "$manifest" "$manager" "$source" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/after-pending-apply.out"
grep -Fq 'Combined setup complete' "$tmp/after-pending-apply.out"
test -f "$game/.intimatr-managed-launch/speed.toml"

echo 'Full setup fixture: recovered completed and pending stages, tamper guards, and no Wine launch passed'
