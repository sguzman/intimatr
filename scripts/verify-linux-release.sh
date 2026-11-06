#!/usr/bin/env bash
# Independently verify a packaged Linux ELF + Windows PE bundle.
set -euo pipefail

cd "$(dirname "$0")/.."
archive="${1:-}"
version="${2:-}"
if [[ -z "$archive" || ! "$version" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,79}$ ]]; then
  echo "Usage: bash scripts/verify-linux-release.sh ARCHIVE VERSION" >&2
  exit 2
fi
if [[ ! -s "$archive" || ! -s "$archive.sha256" ]]; then
  echo "Missing Linux/Wine release archive or SHA256 sidecar" >&2
  exit 1
fi
name="intimatr-linux-wine-x86_64-$version"
if [[ "$(basename "$archive")" != "$name.tar.gz" ]]; then
  echo "Archive filename/version mismatch" >&2
  exit 1
fi
(
  cd "$(dirname "$archive")"
  sha256sum -c "$(basename "$archive").sha256"
)
stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
tar -tzf "$archive" | while IFS= read -r member; do
  case "$member" in
    "$name"|"$name/"|"$name/"*) ;;
    *) echo "Unexpected archive entry: $member" >&2; exit 1 ;;
  esac
  if [[ "$member" == *"/../"* || "$member" == *"/.." ]]; then
    echo "Parent directory traversal in archive" >&2
    exit 1
  fi
done
tar -xzf "$archive" -C "$stage"
root="$stage/$name"
if [[ -n "$(find "$root" -type l -print -quit)" ]]; then
  echo "Linux/Wine archive contains a symbolic link" >&2
  exit 1
fi
for file in bin/intimatr-bottles wine/intimatr.dll wine/intimatr-speed.exe \
  wine/wine-process-list.exe wine/wine-game-attach-helper.exe \
  config/speed-overrides.example.toml \
  config/ExampleGame.exe.toml README.md LICENSE docs/linux-wine-build.md \
  docs/rpc.md docs/speed-control-roadmap.md \
  docs/bottles-multi-game.md docs/performance-hud.md docs/speed-profiles.md \
  config/bottles-proxy-template.toml \
  scripts/bottles-discover.sh scripts/bottles-multi-game.sh \
  scripts/bottles-hud.sh scripts/generate-steam-api64-forwarders.sh \
  native/launch_proxy/steam_api64_bridge.c VERSION.txt SHA256SUMS.txt; do
  test -s "$root/$file" || { echo "Missing release entry: $file" >&2; exit 1; }
done
(
  cd "$root"
  sha256sum -c SHA256SUMS.txt
)
for script in bottles-discover.sh bottles-multi-game.sh bottles-hud.sh generate-steam-api64-forwarders.sh; do
  test -x "$root/scripts/$script" || {
    echo "Packaged deployment script is not executable: $script" >&2
    exit 1
  }
  bash -n "$root/scripts/$script"
done
grep -Fq 'MIT License' "$root/LICENSE"
grep -Fxq "Version: $version" "$root/VERSION.txt"
file "$root/bin/intimatr-bottles" | grep -q 'ELF 64-bit'
file "$root/wine/intimatr.dll" | grep -Eq 'PE32[+].*DLL.*x86-64'
for exe in intimatr-speed.exe wine-process-list.exe wine-game-attach-helper.exe; do
  file "$root/wine/$exe" | grep -Eq 'PE32[+].*x86-64'
done
echo "Verified Linux/Wine mixed-architecture bundle: $archive"
