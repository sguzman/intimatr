#!/usr/bin/env bash
# Read-only fixture for exact native Bottles registration discovery.
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$here"
for tool in jq grep; do command -v "$tool" >/dev/null; done
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-discovery-test.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/root/Alpha/drive_c/Games/Example"
game="$tmp/root/Alpha/drive_c/Games/Example"
printf 'PE fixture\n' >"$game/Example.exe"
printf 'Steam DLL fixture\n' >"$game/steam_api64.dll"
cat >"$tmp/bin/x86_64-w64-mingw32-objdump" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == -p && "$2" == *.exe ]] || exit 2
printf '%s\n' '  DLL Name: steam_api64.dll'
EOF
cat >"$tmp/bin/intimatr-bottles" <<'EOF'
#!/usr/bin/env bash
[[ "$*" == "bottle-compat --all" ]] || exit 2
cat "$MOCK_COMPAT"
EOF
chmod 0755 "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
export MOCK_COMPAT="$tmp/compat.json"
jq -n --arg game "$game" \
  '{
    operation:"read_only_all_registered_bottles_static_pe_compatibility",
    bottles:[{
      bottle:"Alpha",bottle_prefix_association_verified:true,
      programs:[
        {registration_metadata_valid:true,
         static_pe_compatibility:{static_import_status:"static_four_timer_names_present"},
         resolved_linux_file:($game+"/Example.exe"),
         registered_name:"Example Offline",
         registered_windows_path:"C:\\Games\\Example\\Example.exe"},
        {registration_metadata_valid:true,
         static_pe_compatibility:{static_import_status:"static_timer_imports_missing"},
         resolved_linux_file:($game+"/Other.exe"),
         registered_name:"Unsupported",
         registered_windows_path:"C:\\Games\\Example\\Other.exe"}
      ]
    }]
  }' >"$MOCK_COMPAT"
bash -n scripts/bottles-discover.sh
bash scripts/bottles-discover.sh "$tmp/bin/intimatr-bottles" "$tmp/selection.json" \
  >"$tmp/result.log"
grep -Fq 'Wrote 1 read-only candidates' "$tmp/result.log"
jq -e --arg game "$game" \
  '.version == 1 and (.games | length == 1) and
   .games[0].game_dir == $game and
   .games[0].executable == "Example.exe" and
   .games[0].bottle == "Alpha" and
   .games[0].renderer == "auto" and
   .games[0].registered_program == "Example Offline" and
   .games[0].windows_executable == "C:\\Games\\Example\\Example.exe"' \
  "$tmp/selection.json" >/dev/null
test -f "$game/steam_api64.dll"
test ! -e "$game/MangoHud.conf"
if bash scripts/bottles-discover.sh "$tmp/bin/intimatr-bottles" "$tmp/selection.json" \
  >"$tmp/duplicate.out" 2>"$tmp/duplicate.err"; then
  echo "Discovery overwrote a reviewed selection" >&2
  exit 1
fi
echo 'Bottles discovery fixture: exact registered identity preserved and no overwrite'
