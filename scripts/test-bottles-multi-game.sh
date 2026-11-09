#!/usr/bin/env bash
# Isolated DLL batch rollback regression: no Wine, game or real PE modifications.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for tool in jq sha256sum awk grep; do command -v "$tool" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-proxy-rollback-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/mockbin"
for name in First Second; do
  game="$tmp/$name"
  mkdir -p "$game"
  printf 'mock game\n' >"$game/Game.exe"
  printf 'original mock Steam DLL\n' >"$game/steam_api64.dll"
done
export MOCK_ROOT="$tmp"
export MOCK_EVENTS="$tmp/events"

cat >"$tmp/mockbin/x86_64-w64-mingw32-objdump" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == -p && $# == 2 ]] || exit 2
case "$2" in
  "$MOCK_ROOT"/*/Game.exe)
    echo '  DLL Name: steam_api64.dll'
    ;;
  "$MOCK_ROOT"/*/steam_api64.dll)
    printf '%s\n' 'NumberOfFunctions 1' 'NumberOfNames 1' \
      'Ordinal Base 1' '[Ordinal/Name Pointer] Table' '  [0] TestExport'
    ;;
  *)
    printf '%s\n' 'DLL Name: intimatr.dll' \
      'steam_api64_intimatr_original.TestExport'
    ;;
esac
EOF
cat >"$tmp/mockbin/x86_64-w64-mingw32-dlltool" <<'EOF'
#!/usr/bin/env bash
while (( $# )); do
  if [[ "$1" == --output-lib ]]; then
    printf 'mock import library\n' >"$2"
    exit 0
  fi
  shift
done
exit 2
EOF
cat >"$tmp/mockbin/x86_64-w64-mingw32-gcc" <<'EOF'
#!/usr/bin/env bash
while (( $# )); do
  if [[ "$1" == -o ]]; then
    printf 'mock proxy\n' >"$2"
    exit 0
  fi
  shift
done
exit 2
EOF
cat >"$tmp/mockbin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == launch-proxy ]] || exit 3
action="$2"
[[ "$3" == --game-dir ]] || exit 3
game="$4"
printf '%s|%s\n' "$action" "$game" >>"$MOCK_EVENTS"
case "$action" in
  status)
    if [[ -e "$game/intimatr-launch-deployment.json" ]]; then
      printf '{"status":"installed","original_preserved":true}\n'
    else
      printf '{"status":"not_installed","original_preserved":false}\n'
    fi
    ;;
  install)
    if [[ "$game" == "$MOCK_ROOT/Second" ]]; then
      echo 'Simulated second-game installation failure' >&2
      exit 44
    fi
    printf '{}\n' >"$game/intimatr-launch-deployment.json"
    ;;
  remove)
    rm -- "$game/intimatr-launch-deployment.json"
    ;;
  *) exit 3 ;;
esac
EOF
chmod 0755 "$tmp/mockbin/"*
export PATH="$tmp/mockbin:$PATH"
manifest="$tmp/selection.json"
jq -n --arg a "$tmp/First" --arg b "$tmp/Second" \
  '{version:1,games:[{game_dir:$a,executable:"Game.exe"},
                      {game_dir:$b,executable:"Game.exe"}]}' >"$manifest"
printf 'mock runtime\n' >"$tmp/intimatr.dll"
manager="$tmp/mockbin/intimatr-bottles"
before_first="$(sha256sum "$tmp/First/steam_api64.dll" | cut -d' ' -f1)"
before_second="$(sha256sum "$tmp/Second/steam_api64.dll" | cut -d' ' -f1)"

bash -n scripts/bottles-multi-game.sh
if bash scripts/bottles-multi-game.sh install "$manifest" "$manager" \
  "$tmp/intimatr.dll" --offline-single-player --games-closed \
  >"$tmp/install.out" 2>"$tmp/install.err"; then
  echo 'Expected second-game install failure was not detected' >&2
  exit 1
fi

grep -Fq 'Simulated second-game installation failure' "$tmp/install.err"
grep -Fq "Restored newly installed proxy: $tmp/First" "$tmp/install.err"
grep -Fxq "install|$tmp/First" "$MOCK_EVENTS"
grep -Fxq "install|$tmp/Second" "$MOCK_EVENTS"
grep -Fxq "remove|$tmp/First" "$MOCK_EVENTS"
if grep -Fxq "remove|$tmp/Second" "$MOCK_EVENTS"; then
  echo 'Rollback tried to remove failed, unverified second install' >&2
  exit 1
fi
test ! -e "$tmp/First/intimatr-launch-deployment.json"
test ! -e "$tmp/Second/intimatr-launch-deployment.json"
test "$before_first" = "$(sha256sum "$tmp/First/steam_api64.dll" | cut -d' ' -f1)"
test "$before_second" = "$(sha256sum "$tmp/Second/steam_api64.dll" | cut -d' ' -f1)"
echo 'DLL multi-game fixture: failed second install restored only first verified owned proxy'
