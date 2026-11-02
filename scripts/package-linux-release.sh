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
  "$windows/wine-process-list.exe" config/speed-overrides.example.toml \
  config/ExampleGame.exe.toml README.md docs/linux-wine-build.md \
  docs/rpc.md docs/speed-control-roadmap.md; do
  if [[ ! -s "$file" || ! -f "$file" ]]; then
    echo "Required Linux/Wine release input missing or empty: $file" >&2
    exit 1
  fi
done

stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/bin" "$root/wine" "$root/config" "$root/docs"
install -m 0755 "$native" "$root/bin/intimatr-bottles"
install -m 0644 "$windows/intimatr.dll" "$root/wine/intimatr.dll"
install -m 0644 "$windows/intimatr-speed.exe" "$root/wine/intimatr-speed.exe"
install -m 0644 "$windows/wine-process-list.exe" "$root/wine/wine-process-list.exe"
install -m 0644 config/speed-overrides.example.toml "$root/config/"
install -m 0644 config/ExampleGame.exe.toml "$root/config/"
install -m 0644 README.md "$root/README.md"
for doc in linux-wine-build.md rpc.md speed-control-roadmap.md; do
  install -m 0644 "docs/$doc" "$root/docs/$doc"
done
cat > "$root/VERSION.txt" <<EOF
Intimatr Linux/Wine bundle
Version: $version
Linux: x86_64 ELF
Wine: x86_64 Windows GNU PE
Scope: offline/single-player testing only
Status: automatic game enrollment and production-safe hooks NOT verified
EOF
(
  cd "$root"
  sha256sum \
    bin/intimatr-bottles \
    wine/intimatr.dll wine/intimatr-speed.exe wine/wine-process-list.exe \
    config/speed-overrides.example.toml config/ExampleGame.exe.toml \
    README.md docs/linux-wine-build.md docs/rpc.md \
    docs/speed-control-roadmap.md VERSION.txt > SHA256SUMS.txt
)
tar -czf "$archive" -C "$stage" "$name"
(
  cd dist
  sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
)
printf '%s\n' "$archive"
