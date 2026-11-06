#!/usr/bin/env bash
# Reproducible-layout release archive, containing native ELF and Windows PE.
# Packaging does not enroll games, inject DLLs or change any Bottles data.
set -euo pipefail

cd "$(dirname "$0")/.."
version="${1:-}"
if [[ ! "$version" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,79}$ ]]; then
  echo "Usage: bash scripts/package-linux-release.sh VERSION" >&2
  exit 2
fi

name="intimatr-linux-wine-x86_64-$version"
archive="dist/$name.tar.gz"
native="target/release/intimatr-bottles"
windows="target/x86_64-pc-windows-gnu/release"
mkdir -p dist
for file in "$native" "$windows/intimatr.dll" "$windows/intimatr-speed.exe" \
  "$windows/wine-process-list.exe" "$windows/wine-game-attach-helper.exe" \
  config/speed-overrides.example.toml \
  config/ExampleGame.exe.toml README.md docs/linux-wine-build.md \
  docs/rpc.md docs/speed-control-roadmap.md \
  docs/bottles-multi-game.md docs/performance-hud.md docs/speed-profiles.md \
  config/bottles-proxy-template.toml \
  scripts/bottles-discover.sh scripts/bottles-multi-game.sh \
  scripts/bottles-hud.sh scripts/generate-steam-api64-forwarders.sh \
  native/launch_proxy/steam_api64_bridge.c; do
  if [[ ! -s "$file" || ! -f "$file" ]]; then
    echo "Required Linux/Wine release input missing or empty: $file" >&2
    exit 1
  fi
done

stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/bin" "$root/wine" "$root/config" "$root/docs" \
  "$root/scripts" "$root/native/launch_proxy"
install -m 0755 "$native" "$root/bin/intimatr-bottles"
install -m 0644 "$windows/intimatr.dll" "$root/wine/intimatr.dll"
install -m 0644 "$windows/intimatr-speed.exe" "$root/wine/intimatr-speed.exe"
install -m 0644 "$windows/wine-process-list.exe" "$root/wine/wine-process-list.exe"
install -m 0644 "$windows/wine-game-attach-helper.exe" "$root/wine/wine-game-attach-helper.exe"
install -m 0644 config/speed-overrides.example.toml "$root/config/"
install -m 0644 config/ExampleGame.exe.toml "$root/config/"
install -m 0644 config/bottles-proxy-template.toml "$root/config/"
install -m 0644 README.md "$root/README.md"
for script in bottles-discover.sh bottles-multi-game.sh bottles-hud.sh generate-steam-api64-forwarders.sh; do
  install -m 0755 "scripts/$script" "$root/scripts/$script"
done
install -m 0644 native/launch_proxy/steam_api64_bridge.c "$root/native/launch_proxy/"
for doc in linux-wine-build.md rpc.md speed-control-roadmap.md \
  bottles-multi-game.md performance-hud.md speed-profiles.md; do
  install -m 0644 "docs/$doc" "$root/docs/$doc"
done
cat > "$root/VERSION.txt" <<EOF
Intimatr Linux/Wine bundle
Version: $version
Linux: x86_64 ELF
Wine: x86_64 Windows GNU PE
Scope: offline/single-player testing only
Status: experimental opt-in offline-game attach and startup proxy; multi-game/HUD field QA incomplete
EOF
(
  cd "$root"
  sha256sum \
    bin/intimatr-bottles \
    wine/intimatr.dll wine/intimatr-speed.exe wine/wine-process-list.exe \
    wine/wine-game-attach-helper.exe \
    config/speed-overrides.example.toml config/ExampleGame.exe.toml \
    config/bottles-proxy-template.toml \
    scripts/bottles-discover.sh scripts/bottles-multi-game.sh \
    scripts/bottles-hud.sh scripts/generate-steam-api64-forwarders.sh \
    native/launch_proxy/steam_api64_bridge.c \
    README.md docs/linux-wine-build.md docs/rpc.md \
    docs/speed-control-roadmap.md docs/bottles-multi-game.md \
    docs/performance-hud.md docs/speed-profiles.md VERSION.txt > SHA256SUMS.txt
)
tar -czf "$archive" -C "$stage" "$name"
(
  cd dist
  sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
)
printf '%s\n' "$archive"
