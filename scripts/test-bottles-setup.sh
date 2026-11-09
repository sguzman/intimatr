#!/usr/bin/env bash
# Isolated fixture for the combined Bottles coordinator; no real Bottles/Wine.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for tool in jq sha256sum realpath; do command -v "$tool" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-bottles-setup-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/mockbin" "$tmp/root/Eastward/drive_c/Games/Eastward"
game="$tmp/root/Eastward/drive_c/Games/Eastward"
printf 'fixture executable\n' >"$game/Eastward.exe"
printf '{}\n' >"$game/intimatr-launch-deployment.json"
printf 'fixture DLL\n' >"$game/intimatr.dll"
before="$(sha256sum "$game/intimatr.dll" | cut -d' ' -f1)"

cat >"$tmp/mockbin/bottles-cli" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "--json info bottles-path") printf '%s\n' "$MOCK_BOTTLES_ROOT" ;;
  "--json list bottles") printf '{"Eastward":{},"Other":{}}\n' ;;
  *) echo "unexpected Bottles CLI: $*" >&2; exit 3 ;;
esac
EOF

cat >"$tmp/mockbin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  bottle-env)
    printf '%s\n' "$*" >>"$MOCK_EVENTS"
    if [[ -n "${MOCK_MUTATE_MANIFEST:-}" ]]; then
      printf '%s\n' '{"version":1,"games":[{"game_dir":"/outside","executable":"Outside.exe"}]}' > "$MOCK_MUTATE_MANIFEST"
    fi
    if [[ "$2" == status ]]; then
      printf '{"operation":"status","bottle":"Eastward","ownership":"unmanaged","allowlist_entries_to_add":["MANGOHUD"]}\n'
      if [[ " $* " == *" --all "* ]]; then
        printf '{"operation":"status","bottle":"Other","ownership":"unmanaged","allowlist_entries_to_add":["MANGOHUD"]}\n'
      fi
    else
      printf '{"operation":"mock_bottle_env","files_modified":false}\n'
    fi
    ;;
  launch-proxy)
    [[ "$3" == --game-dir ]]
    if [[ "$2" == remove ]]; then
      printf 'remove|%s\n' "$4" >>"$MOCK_EVENTS"
      rm -- "$4/intimatr-launch-deployment.json"
    elif [[ "$2" == status ]]; then
      if [[ "$4" == *"/Uninstalled" &&
            ! -f "$4/intimatr-launch-deployment.json" ]]; then
        printf '{"status":"not_installed","original_preserved":false}\n'
      else
        printf '{"status":"installed","original_preserved":true}\n'
      fi
    else
      exit 3
    fi
    ;;
  *) echo "unexpected Intimatr command: $*" >&2; exit 3 ;;
esac
EOF
cat >"$tmp/mockbin/x86_64-w64-mingw32-objdump" <<'EOF'
#!/usr/bin/env bash
# A fixture import table: supported offline game only.
[[ "$2" == *"/Uninstalled/Uninstalled.exe" ]] &&
  printf '%s\n' '  DLL Name: steam_api64.dll'
EOF
chmod 0755 "$tmp/mockbin/x86_64-w64-mingw32-objdump"
chmod 0755 "$tmp/mockbin/bottles-cli" "$tmp/mockbin/intimatr-bottles"
export PATH="$tmp/mockbin:$PATH"
export MOCK_BOTTLES_ROOT="$tmp/root"
export MOCK_EVENTS="$tmp/events"
manager="$tmp/mockbin/intimatr-bottles"
manifest="$tmp/selection.json"
jq -n --arg d "$game" '{version:1,games:[{game_dir:$d,executable:"Eastward.exe"}]}' >"$manifest"

bash -n scripts/bottles-setup.sh
bash scripts/bottles-setup.sh plan "$manifest" "$manager" --all-bottles >"$tmp/plan.log"
grep -Fxq 'Read-only plan: no bottle, DLL or HUD files changed.' "$tmp/plan.log"
grep -Fxq 'bottle-env plan --all --preset intimatr-hud' "$MOCK_EVENTS"
test ! -e "$game/MangoHud.conf"
test ! -e "$game/intimatr-mangohud.sha256"

bash scripts/bottles-setup.sh status "$manifest" "$manager" >"$tmp/status.log"
grep -Fxq 'bottle-env status --bottle Eastward' "$MOCK_EVENTS"
grep -Fq 'DLL owned and verified:' "$tmp/status.log"

# A concurrent edit to the source manifest after preflight must never switch
# the selection seen by downstream HUD/DLL installers in the same invocation.
cp -- "$manifest" "$tmp/mutable-selection.json"
MOCK_MUTATE_MANIFEST="$tmp/mutable-selection.json" \
  bash scripts/bottles-setup.sh plan "$tmp/mutable-selection.json" "$manager" \
  >"$tmp/snapshot-plan.log"
grep -Fq "DLL owned and verified: $game" "$tmp/snapshot-plan.log"
grep -Fq 'Selection: 1 games; 0 new DLLs; 1 new HUD configs.' "$tmp/snapshot-plan.log"
jq -e '.games[0].game_dir == "/outside"' "$tmp/mutable-selection.json" >/dev/null
test ! -e "$game/MangoHud.conf"

# A mutation without all acknowledgments must fail before writing.
if bash scripts/bottles-setup.sh apply "$manifest" "$manager" --all-bottles \
  >"$tmp/missing-ack.out" 2>"$tmp/missing-ack.err"; then
  echo "Setup accepted apply without closed/offline acknowledgments" >&2
  exit 1
fi
test ! -e "$game/MangoHud.conf"

# A prior incomplete setup must block new apply BEFORE any environment or
# game modifications. Its journal must be handled explicitly.
unfinished="$(mktemp -d "$tmp/root/.intimatr-setup.XXXXXXXX")"
printf 'dll-started\n' >"$unfinished/stage"
env_before="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS" || true)"
if bash scripts/bottles-setup.sh apply "$manifest" "$manager" \
  --offline-single-player --games-closed --bottles-closed --all-bottles \
  >"$tmp/unfinished.out" 2>"$tmp/unfinished.err"; then
  echo "New apply ignored an unfinished recovery journal" >&2
  exit 1
fi
grep -Fq 'Unfinished Intimatr setup (dll-started)' "$tmp/unfinished.err"
test "$env_before" -eq "$(grep -c '^bottle-env apply ' "$MOCK_EVENTS" || true)"
test ! -e "$game/MangoHud.conf"
rm -- "$unfinished/stage"
rmdir -- "$unfinished"

# Existing compatible Intimatr DLL is preserved; missing HUD is installed.
bash scripts/bottles-setup.sh apply "$manifest" "$manager" \
  --offline-single-player --games-closed --bottles-closed --all-bottles \
  >"$tmp/apply.log"
grep -Fxq 'bottle-env apply --all --preset intimatr-hud --bottles-closed' "$MOCK_EVENTS"
test -f "$game/MangoHud.conf"
test -f "$game/intimatr-mangohud.sha256"
journals=("$tmp/root"/.intimatr-setup.*)
test "${#journals[@]}" -eq 1
journal="${journals[0]}"
test -d "$journal"
test "$(stat -c %a "$journal")" = 700
test "$(cat "$journal/stage")" = complete
jq -e '.version == 1 and .all_bottles == true' "$journal/metadata.json" >/dev/null
jq -e --arg expected "$game" '.games[0].game_dir == $expected' "$journal/selection.json" >/dev/null
jq -e --arg expected "$game" '.games[0].game_dir == $expected' "$journal/new-hud.json" >/dev/null
jq -s -e 'length == 2 and all(.[]; .operation == "status" and .ownership == "unmanaged")' \
  "$journal/env-before.jsonl" >/dev/null
test "$before" = "$(sha256sum "$game/intimatr.dll" | cut -d' ' -f1)"
managed="$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"

# Idempotent apply skips an already managed, unchanged HUD.
bash scripts/bottles-setup.sh apply "$manifest" "$manager" \
  --offline-single-player --games-closed --bottles-closed --all-bottles \
  >"$tmp/reapply.log"
grep -Fq '0 new HUD configs' "$tmp/reapply.log"
test "$managed" = "$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"

# A modified HUD is a hard refusal. Do not overwrite it or the DLL.
printf '# local user customization\n' >>"$game/MangoHud.conf"
modified="$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"
if bash scripts/bottles-setup.sh plan "$manifest" "$manager" \
  >"$tmp/tampered.out" 2>"$tmp/tampered.err"; then
  echo "Setup accepted a modified HUD config" >&2
  exit 1
fi
grep -q 'Refusing unmanaged or modified MangoHud' "$tmp/tampered.err"
test "$modified" = "$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"
test "$before" = "$(sha256sum "$game/intimatr.dll" | cut -d' ' -f1)"

# Even a reviewed manifest cannot select a path outside native Bottles root.
mkdir -p "$tmp/not-a-bottle"
printf 'file\n' >"$tmp/not-a-bottle/Outside.exe"
jq -n --arg d "$tmp/not-a-bottle" \
  '{version:1,games:[{game_dir:$d,executable:"Outside.exe"}]}' \
  >"$tmp/outside.json"
if bash scripts/bottles-setup.sh plan "$tmp/outside.json" "$manager" --all-bottles \
  >"$tmp/outside.out" 2>"$tmp/outside.err"; then
  echo "Setup accepted a directory outside native Bottles root" >&2
  exit 1
fi
grep -q 'does not resolve literally under native Bottles root' "$tmp/outside.err"

# All-bottles environment-only plan does not enroll or touch any games.
jq -n '{version:1,games:[]}' >"$tmp/empty.json"
bash scripts/bottles-setup.sh plan "$tmp/empty.json" "$manager" \
  --all-bottles >"$tmp/empty-plan.log"
grep -Fq 'Selection: 0 games' "$tmp/empty-plan.log"

# New DLL installs get a real read-only compatibility preflight, but an apply
# without an explicitly selected Windows DLL must refuse BEFORE env writes.
new_game="$tmp/root/Other/drive_c/Games/Uninstalled"
mkdir -p "$new_game"
printf 'fake PE fixture\n' >"$new_game/Uninstalled.exe"
printf 'fake steam library\n' >"$new_game/steam_api64.dll"
jq -n --arg d "$new_game" \
  '{version:1,games:[{game_dir:$d,executable:"Uninstalled.exe"}]}' >"$tmp/new.json"
bash scripts/bottles-setup.sh plan "$tmp/new.json" "$manager" >"$tmp/new-plan.log"
grep -Fq '1 new DLLs' "$tmp/new-plan.log"
grep -Fq 'Candidate (read only):' "$tmp/new-plan.log"
before_apply_count="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS")"
if bash scripts/bottles-setup.sh apply "$tmp/new.json" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/missing-runtime.out" 2>"$tmp/missing-runtime.err"; then
  echo "Setup tried a new DLL installation without an explicit runtime" >&2
  exit 1
fi
grep -Fq 'requires --runtime' "$tmp/missing-runtime.err"
after_apply_count="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS")"
test "$before_apply_count" -eq "$after_apply_count"
test ! -e "$new_game/intimatr.dll"

# An existing but invalid x64 Windows runtime must be rejected BEFORE any
# inherited environment allowlists are modified. Mock the presence of both
# MinGW build commands so the failure exercises PE validation, not tool lookup.
cat >"$tmp/mockbin/x86_64-w64-mingw32-gcc" <<'EOF'
#!/usr/bin/env bash
exit 99
EOF
cat >"$tmp/mockbin/x86_64-w64-mingw32-dlltool" <<'EOF'
#!/usr/bin/env bash
exit 99
EOF
chmod 0755 "$tmp/mockbin/x86_64-w64-mingw32-gcc" "$tmp/mockbin/x86_64-w64-mingw32-dlltool"
printf 'not a Windows PE DLL\n' >"$tmp/invalid-runtime.dll"
before_apply_count="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS")"
if bash scripts/bottles-setup.sh apply "$tmp/new.json" "$manager" \
  --runtime "$tmp/invalid-runtime.dll" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/invalid-runtime.out" 2>"$tmp/invalid-runtime.err"; then
  echo "Setup applied invalid x64 Windows runtime" >&2
  exit 1
fi
grep -Fq 'Runtime is not a readable x64 Windows PE DLL' "$tmp/invalid-runtime.err"
after_apply_count="$(grep -c '^bottle-env apply ' "$MOCK_EVENTS")"
test "$before_apply_count" -eq "$after_apply_count"
test ! -e "$new_game/intimatr.dll"

# The all-bottles scope with an empty manifest is environment-only, never
# an implicit request to deploy DLLs to every game in that inventory.
bash scripts/bottles-setup.sh apply "$tmp/empty.json" "$manager" \
  --offline-single-player --games-closed --bottles-closed --all-bottles \
  >"$tmp/empty-apply.log"
grep -Fq 'Selection: 0 games; 0 new DLLs; 0 new HUD configs.' "$tmp/empty-apply.log"
test ! -e "$new_game/intimatr.dll"

# Recovery is read-only until explicitly approved. A user-modified HUD
# blocks the entire rollback; the original Intimatr DLL remains unchanged.
bash scripts/bottles-setup-recover.sh status "$journal" "$manager" >"$tmp/recovery-status.out"
grep -Fq 'Blockers: 1.' "$tmp/recovery-status.out"
if bash scripts/bottles-setup-recover.sh rollback "$journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed \
  >"$tmp/blocked-recovery.out" 2>"$tmp/blocked-recovery.err"; then
  echo "Recovery overwrote a locally modified HUD" >&2
  exit 1
fi
grep -Fq 'Refusing partial rollback while blockers remain' "$tmp/blocked-recovery.err"
test "$modified" = "$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"
test "$(cat "$journal/stage")" = complete

# Restoring the exact unchanged managed HUD allows a selective rollback.
# Its existing, pre-installed Intimatr DLL must never be removed.
sed -i '$d' "$game/MangoHud.conf"
test "$managed" = "$(sha256sum "$game/MangoHud.conf" | cut -d' ' -f1)"
bash scripts/bottles-setup-recover.sh rollback "$journal" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/recovered.out"
test "$(cat "$journal/stage")" = reverted
test ! -e "$game/MangoHud.conf"
test ! -e "$game/intimatr-mangohud.sha256"
test "$before" = "$(sha256sum "$game/intimatr.dll" | cut -d' ' -f1)"
if grep -Fq "remove|$game" "$MOCK_EVENTS"; then
  echo "Recovery removed a pre-existing DLL" >&2
  exit 1
fi

# Simulate a previous invocation interrupted during the DLL stage.
# It must restore only the newly managed proxy, not the original Steam DLL.
journal_dll="$(mktemp -d "$tmp/root/.intimatr-setup.XXXXXXXX")"
cp -- "$tmp/new.json" "$journal_dll/selection.json"
cp -- "$tmp/new.json" "$journal_dll/new-dll.json"
jq -n '{version:1,games:[]}' >"$journal_dll/new-hud.json"
printf '{"operation":"status","bottle":"Other","ownership":"owned_unchanged","allowlist_entries_to_add":[]}\n' >"$journal_dll/env-before.jsonl"
jq -n --arg root "$tmp/root" '{version:1,root:$root,all_bottles:false}' >"$journal_dll/metadata.json"
printf 'dll-started\n' >"$journal_dll/stage"
printf '{}\n' >"$new_game/intimatr-launch-deployment.json"
steam_before="$(sha256sum "$new_game/steam_api64.dll" | cut -d' ' -f1)"
bash scripts/bottles-setup-recover.sh status "$journal_dll" "$manager" >"$tmp/dll-status.out"
grep -Fq '0 HUD, 1 DLL, 0 bottle environments. Blockers: 0.' "$tmp/dll-status.out"
bash scripts/bottles-setup-recover.sh rollback "$journal_dll" "$manager" \
  --offline-single-player --games-closed --bottles-closed >"$tmp/dll-rollback.out"
test "$(cat "$journal_dll/stage")" = reverted
test ! -e "$new_game/intimatr-launch-deployment.json"
grep -Fxq "remove|$new_game" "$MOCK_EVENTS"
test "$steam_before" = "$(sha256sum "$new_game/steam_api64.dll" | cut -d' ' -f1)"
test "$before" = "$(sha256sum "$game/intimatr.dll" | cut -d' ' -f1)"

echo "Combined Bottles fixture: plan, status, snapshot, journal, HUD and DLL recovery, tamper and path refusal passed"
