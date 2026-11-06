#!/usr/bin/env bash
# Batch-install the same Intimatr runtime, with a different ABI-correct Steam
# forwarder per explicitly selected, offline Bottles game. No process injection.
# Uses the audited Rust deployment manager for per-game backups and rollback.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  bash scripts/bottles-multi-game.sh plan MANIFEST_JSON ABSOLUTE_INTIMATR_BOTTLES
  bash scripts/bottles-multi-game.sh install MANIFEST_JSON ABSOLUTE_INTIMATR_BOTTLES ABSOLUTE_INTIMATR_DLL --offline-single-player --games-closed
  bash scripts/bottles-multi-game.sh remove MANIFEST_JSON ABSOLUTE_INTIMATR_BOTTLES --offline-single-player --games-closed

Manifest: {"version":1,"games":[{"game_dir":"/absolute/bottle/drive_c/Games/Game","executable":"Game.exe"},...]}
The selected executable must import a local x64 steam_api64.dll by name.
The runtime DLL is common; each Steam forwarder is generated from its own
original PE export table. An invalid game fails preflight before mutation.
Close *all* selected games before install/remove. Do not use with multiplayer,
anti-cheat games, or programs you don't have permission to modify.
USAGE
  exit 2
}

[[ $# -ge 3 ]] || usage
action="$1"
manifest="$2"
manager="$3"
shift 3
[[ "$action" == plan || "$action" == install || "$action" == remove ]] || usage
[[ "$manifest" == /* && -f "$manifest" && ! -L "$manifest" ]] || { echo "Expected an existing absolute regular manifest" >&2; exit 2; }
[[ "$manager" == /* && -f "$manager" && -x "$manager" && ! -L "$manager" ]] || { echo "Expected an absolute executable Intimatr Bottles manager" >&2; exit 2; }
runtime=""
if [[ "$action" == install ]]; then
  [[ $# == 3 && "$1" == /* && -f "$1" && ! -L "$1" && "$2" == --offline-single-player && "$3" == --games-closed ]] || usage
  runtime="$1"
elif [[ "$action" == remove ]]; then
  [[ $# == 2 && "$1" == --offline-single-player && "$2" == --games-closed ]] || usage
else
  [[ $# == 0 ]] || usage
fi

for tool in jq sha256sum x86_64-w64-mingw32-objdump; do command -v "$tool" >/dev/null || { echo "Missing required tool: $tool" >&2; exit 2; }; done
[[ -r "$manifest" ]] || { echo "Cannot read manifest" >&2; exit 2; }
jq -e '
  type == "object" and .version == 1 and
  (.games | type == "array" and length >= 1 and length <= 32) and
  ([.games[].game_dir] | length == (unique | length)) and
  all(.games[];
    type == "object" and
    (.game_dir | type == "string" and startswith("/") and
      (test("[\\t\\r\\n]") | not)) and
    (.executable | type == "string" and test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$") and
      (contains("..") | not))
  )' "$manifest" >/dev/null || { echo "Invalid, duplicate, or overlarge batch manifest" >&2; exit 2; }

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
generator="$here/scripts/generate-steam-api64-forwarders.sh"
bridge="$here/native/launch_proxy/steam_api64_bridge.c"
template="$here/config/bottles-proxy-template.toml"

if [[ "$action" == install ]]; then
  for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-dlltool; do
    command -v "$tool" >/dev/null || { echo "Missing cross-toolchain: $tool" >&2; exit 2; }
  done
  [[ -f "$generator" && -f "$bridge" && -f "$template" ]] || { echo "Incomplete Intimatr source checkout" >&2; exit 2; }
fi

declare -a games=() exes=() checksums=()
while IFS=$'\t' read -r game exe; do
  [[ -d "$game" && ! -L "$game" && -f "$game/$exe" && ! -L "$game/$exe" ]] || {
    echo "Missing or linked selected game directory/executable: $game/$exe" >&2; exit 1;
  }
  games+=("$game")
  exes+=("$exe")
done < <(jq -r '.games[] | [.game_dir, .executable] | @tsv' "$manifest")
[[ "${#games[@]}" -ge 1 ]] || { echo "Empty game selection" >&2; exit 2; }

# Global preflight: confirm every game before changing the first one.
for i in "${!games[@]}"; do
  game="${games[i]}"
  exe="${exes[i]}"
  if [[ "$action" == remove ]]; then
    "$manager" launch-proxy status --game-dir "$game" |
      jq -e '.status == "installed" and .original_preserved == true' >/dev/null ||
      { echo "Not safely installed: $game" >&2; exit 1; }
    continue
  fi
  if [[ "$action" == install ]]; then
    "$manager" launch-proxy status --game-dir "$game" |
      jq -e '.status == "not_installed"' >/dev/null ||
      { echo "Existing/unmanaged install: $game" >&2; exit 1; }
  fi
  [[ -f "$game/steam_api64.dll" && ! -L "$game/steam_api64.dll" ]] || {
    echo "Missing original steam_api64.dll: $game" >&2; exit 1;
  }
  if ! x86_64-w64-mingw32-objdump -p "$game/$exe" |
      grep -Eiq '^[[:space:]]*DLL Name: steam_api64\.dll[[:space:]]*$'; then
    echo "Selected executable lacks static steam_api64.dll import: $game/$exe" >&2
    exit 1
  fi
  checksums[i]="$(sha256sum "$game/steam_api64.dll" | cut -d' ' -f1)"
done

if [[ "$action" == plan ]]; then
  for i in "${!games[@]}"; do
    printf 'Candidate (read only): %s | %s | original SHA256 %s\n' "${games[i]}" "${exes[i]}" "${checksums[i]}"
  done
  echo "Preflight only. No game files or Wine processes were changed."
  exit 0
fi

if [[ "$action" == remove ]]; then
  for i in "${!games[@]}"; do
    echo "Restoring original DLL: ${games[i]}"
    "$manager" launch-proxy remove --game-dir "${games[i]}" --offline-single-player --game-closed
  done
  echo "Removal attempted for every selected game; check reports above."
  exit 0
fi

# Build each game's own forwarder before mutating ANY selected game.
temp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-bottles-batch.XXXXXXXX")"
trap 'rm -rf -- "$temp"' EXIT
printf 'LIBRARY intimatr.dll\nEXPORTS\nintimatr_lifecycle_state\n' >"$temp/intimatr_import.def"
x86_64-w64-mingw32-dlltool --def "$temp/intimatr_import.def" \
  --dllname intimatr.dll --output-lib "$temp/libintimatr.dll.a"
for i in "${!games[@]}"; do
  game="${games[i]}"
  exe="${exes[i]}"
  mkdir -p "$temp/$i"
  INTIMATR_OBJDUMP=x86_64-w64-mingw32-objdump \
    bash "$generator" "$game/steam_api64.dll" "$temp/$i/forwarders.def"
  x86_64-w64-mingw32-gcc -shared -o "$temp/$i/steam_api64.dll" \
    "$bridge" "$temp/$i/forwarders.def" "$temp/libintimatr.dll.a"
  x86_64-w64-mingw32-objdump -p "$temp/$i/steam_api64.dll" >"$temp/$i/headers.txt"
  grep -Fq 'DLL Name: intimatr.dll' "$temp/$i/headers.txt"
  grep -Fq 'steam_api64_intimatr_original.' "$temp/$i/headers.txt"
  sed "s/__INTIMATR_GAME_EXE__/$exe/g" "$template" >"$temp/$i/$exe.toml"
  echo "Prepared independent proxy for $exe ($(grep -c '=steam_api64_intimatr_original\.' "$temp/$i/forwarders.def") exports)"
done

# Each operation is reversible on its own. If an install fails, previously
# installed games remain explicitly reported, not silently rolled back.
for i in "${!games[@]}"; do
  echo "Installing selected offline game: ${games[i]}"
  "$manager" launch-proxy install \
    --game-dir "${games[i]}" --exe "${exes[i]}" \
    --original-sha256 "${checksums[i]}" \
    --proxy "$temp/$i/steam_api64.dll" \
    --dll "$runtime" \
    --config "$temp/$i/${exes[i]}.toml" \
    --offline-single-player --game-closed
done
echo "All selected proxies installed. Speed needs an accepted launch profile and startup opt-in; installer does not arm timers or start games."
