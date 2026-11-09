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
    printf '{"operation":"mock_bottle_env","files_modified":false}\n"
    ;;
  launch-proxy)
    [[ "$2" == status && "$3" == --game-dir ]]
    printf '{"status":"installed","original_preserved":true}\n'
    ;;
  *) echo "unexpected Intimatr command: $*" >&2; exit 3 ;;
esac
EOF
# Correct the shell-quoted fixture output without using Python.
sed -i 's/printf.*mock_bottle_env.*$/    printf '\''{"operation":"mock_bottle_env","files_modified":false}\\n'\''/' "$tmp/mockbin/intimatr-bottles"
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

# A mutation without all acknowledgments must fail before writing.
if bash scripts/bottles-setup.sh apply "$manifest" "$manager" --all-bottles \
  >"$tmp/missing-ack.out" 2>"$tmp/missing-ack.err"; then
  echo "Setup accepted apply without closed/offline acknowledgments" >&2
  exit 1
fi
test ! -e "$game/MangoHud.conf"

# Existing compatible Intimatr DLL is preserved; missing HUD is installed.
bash scripts/bottles-setup.sh apply "$manifest" "$manager" \
  --offline-single-player --games-closed --bottles-closed --all-bottles \
  >"$tmp/apply.log"
grep -Fxq 'bottle-env apply --all --preset intimatr-hud --bottles-closed' "$MOCK_EVENTS"
test -f "$game/MangoHud.conf"
test -f "$game/intimatr-mangohud.sha256"
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
echo "Combined Bottles fixture: plan, status, allowlist, existing DLL, HUD install, idempotence, tamper and path refusal passed"
