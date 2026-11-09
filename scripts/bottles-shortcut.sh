#!/usr/bin/env bash
# Explicit, ownership-checked desktop entry for ONE prepared offline game.
# No executable is copied into ~/.local/bin or another custom bin path.
set -euo pipefail
usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-shortcut.sh plan GAME_DIR [--experimental-four-timers]
  bash scripts/bottles-shortcut.sh install GAME_DIR --offline-single-player [--experimental-four-timers]
  bash scripts/bottles-shortcut.sh status GAME_DIR
  bash scripts/bottles-shortcut.sh remove GAME_DIR
This manages only a generated .desktop entry in XDG_DATA_HOME/applications.
It never starts a game; clicking an installed entry explicitly launches it.
EOF
  exit 2
}
(( $# >= 2 )) || usage
action="$1"; game="$2"; shift 2
[[ "$action" == plan || "$action" == install || "$action" == status || "$action" == remove ]] || usage
[[ "$game" == /* && -d "$game" && ! -L "$game" &&
   "$(realpath -e -- "$game")" == "$game" ]] ||
  { echo "Selected game path must be an existing canonical directory" >&2; exit 1; }
timers=false; offline=false
while (( $# )); do
  case "$1" in
    --experimental-four-timers)
      [[ ( "$action" == plan || "$action" == install ) && "$timers" == false ]] || usage
      timers=true; shift ;;
    --offline-single-player)
      [[ "$action" == install && "$offline" == false ]] || usage
      offline=true; shift ;;
    *) usage ;;
  esac
done
if [[ "$action" == install && "$offline" != true ]]; then usage; fi
for tool in sha256sum cmp realpath jq; do
  command -v "$tool" >/dev/null || { echo "Missing required command: $tool" >&2; exit 2; }
done
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
managed="$here/scripts/bottles-managed-launch.sh"
[[ -f "$managed" && ! -L "$managed" ]] ||
  { echo "Missing Intimatr managed launch script" >&2; exit 1; }

# Desktop Exec arguments are quoted, not evaluated by a shell. Fail closed
# for strings requiring special Desktop Entry escaping that we do not perform.
# This avoids introducing shell or field-code interpretation into a launcher.
for path in "$game" "$managed"; do
  case "$path" in
    *$'\n'*|*$'\r'*|*$'\t'*|*'%'*|*'$'*|*'\x60'*|*'"'*|*'\'*)
      echo "Unsupported character in desktop launch path; refusing unsafe shortcut" >&2
      exit 1 ;;
  esac
done

xdg_home="${XDG_DATA_HOME:-${HOME:-}/.local/share}"
[[ "$xdg_home" == /* && "$xdg_home" != *$'\n'* ]] ||
  { echo "XDG_DATA_HOME must be an absolute directory" >&2; exit 1; }
applications="$xdg_home/applications"
id="$(printf '%s' "$game" | sha256sum | cut -d' ' -f1)"
shortcut="$applications/intimatr-bottles-$id.desktop"
name="$(basename -- "$game")"
# The optional timer opt-in is persisted explicitly by the owner. It is not
# enabled just because a managed game profile was prepared.
extra=""
[[ "$timers" == false ]] || extra=" --experimental-four-timers"
generate_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=Intimatr - $name
Comment=Launch a reviewed offline game using its managed Intimatr profile
Exec=/usr/bin/bash "$managed" launch "$game" --offline-single-player$extra
Terminal=false
Icon=applications-games
Categories=Game;
StartupNotify=false
EOF
}
tmp="$(mktemp -d "${TMPDIR:-/tmp}/intimatr-desktop-entry.XXXXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
generate_entry >"$tmp/expected.desktop"
matches_expected() {
  [[ -f "$shortcut" && ! -L "$shortcut" ]] &&
    cmp -s -- "$tmp/expected.desktop" "$shortcut"
}
if [[ "$action" == status || "$action" == remove ]]; then
  # The user may have removed the profile already. Reversal depends on only
  # the exact .desktop content, not the continued existence of game assets.
  if [[ ! -e "$shortcut" && ! -L "$shortcut" ]]; then
    [[ "$action" == status ]] &&
      jq -n --arg path "$shortcut" '{operation:"shortcut_status",path:$path,integrity:"absent"}'
    if [[ "$action" == remove ]]; then echo "Desktop entry already absent: $shortcut"; fi
    exit 0
  fi
  # Existing entry could have been installed with either explicit timer mode.
  # Never delete a user edit and never require repeating install-time options.
  if ! matches_expected; then
    if [[ "$timers" == false ]]; then
      extra=" --experimental-four-timers"
      generate_entry >"$tmp/expected.desktop"
    fi
  fi
  if ! matches_expected; then
    if [[ "$action" == status ]]; then
      jq -n --arg path "$shortcut" \
        '{operation:"shortcut_status",path:$path,integrity:"modified_or_unmanaged"}'
      exit 0
    fi
    echo "Existing desktop entry differs from Intimatr-owned contents; refusing removal" >&2
    exit 1
  fi
  if [[ "$action" == status ]]; then
    jq -n --arg path "$shortcut" \
      '{operation:"shortcut_status",path:$path,integrity:"owned_unchanged"}'
  else
    rm -- "$shortcut"
    echo "Removed only unchanged Intimatr desktop entry: $shortcut"
  fi
  exit 0
fi

# All plan/install choices must pass the existing complete read-only launch
# preflight. This rejects stale policy, modified HUD, wrong registration and
# missing OpenGL wrapper without starting Bottles or Wine.
check_args=(check "$game" --offline-single-player)
[[ "$timers" == false ]] || check_args+=(--experimental-four-timers)
bash "$managed" "${check_args[@]}" >/dev/null
if [[ -e "$shortcut" || -L "$shortcut" ]]; then
  matches_expected ||
    { echo "Existing desktop entry is modified, foreign, or uses another timer mode" >&2; exit 1; }
  echo "Intimatr desktop entry already installed unchanged: $shortcut"
  exit 0
fi
if [[ "$action" == plan ]]; then
  jq -n --arg path "$shortcut" --arg game "$game" \
    '{operation:"shortcut_plan",game_dir:$game,destination:$path,
      creates_new_desktop_entry:true,game_started:false}'
  exit 0
fi
umask 077
mkdir -p -- "$applications"
# Bash noclobber rejects existing destinations and symlinks even if they
# appeared after the preflight. No arbitrary .desktop file is overwritten.
( set -C; cat -- "$tmp/expected.desktop" >"$shortcut" ) ||
  { echo "Desktop destination appeared or is not writable; refusing overwrite" >&2; exit 1; }
chmod 0644 -- "$shortcut"
echo "Installed one Intimatr desktop entry: $shortcut"
