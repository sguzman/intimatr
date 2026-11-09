#!/usr/bin/env bash
# One selected, reviewed Bottles game's native profile/HUD launch.
# This does not modify game files, bottle settings, or registry entries.
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/bottles-launch.sh (check|launch) ABSOLUTE_MANIFEST ABSOLUTE_INTIMATR_MANAGER ABSOLUTE_GAME_DIR ABSOLUTE_SOURCE_TOML ABSOLUTE_DELIVERED_PROFILE [--renderer auto|opengl|vulkan] [--experimental-four-timers] --offline-single-player

The reviewed manifest must include bottle, registered_program, and
windows_executable from recent bottles-discover.sh output.
An auto renderer is used ONLY when a static PE import unambiguously
identifies OpenGL or Vulkan; otherwise supply an explicit renderer.
No auto renderer choice is claimed to prove a visible in-game overlay.
EOF
  exit 2
}

(( $# >= 7 )) || usage
action="$1"; manifest="$2"; manager="$3"; chosen="$4"
source="$5"; profile="$6"; shift 6
[[ "$action" == check || "$action" == launch ]] || usage
for path in "$manifest" "$manager" "$source" "$profile"; do
  [[ "$path" == /* && -f "$path" && ! -L "$path" ]] || usage
done
[[ "$manager" == /* && -x "$manager" ]] || usage
[[ "$chosen" == /* && -d "$chosen" && ! -L "$chosen" ]] || usage
renderer=auto; offline=false; timers=false; renderer_set=false
while (( $# )); do
  case "$1" in
    --renderer)
      [[ "$renderer_set" == false && $# -ge 2 ]] || usage
      renderer="$2"; renderer_set=true; shift 2 ;;
    --offline-single-player)
      [[ "$offline" == false ]] || usage
      offline=true; shift ;;
    --experimental-four-timers)
      [[ "$timers" == false ]] || usage
      timers=true; shift ;;
    *) usage ;;
  esac
done
[[ "$offline" == true && ( "$renderer" == auto || "$renderer" == opengl || "$renderer" == vulkan ) ]] || usage
for cmd in jq bottles-cli realpath; do
  command -v "$cmd" >/dev/null || { echo "Missing required command: $cmd" >&2; exit 2; }
done

# Check only a single exact, reviewed selection. Duplicate entries or missing
# registered identities are hard errors; never fall back to name guessing.
selection="$(jq -ce --arg game "$chosen" '
  if type != "object" or .version != 1 or (.games | type) != "array" then
    error("not a version-1 reviewed manifest")
  else
    [.games[] | select(.game_dir == $game)] as $selected
    | if ($selected | length) != 1 then
        error("expected exactly one selected game")
      else $selected[0] end
  end
' "$manifest")" || { echo "Selected game absent or ambiguous in manifest" >&2; exit 1; }
jq -e '(.bottle | type == "string" and length > 0) and
       (.registered_program | type == "string" and length > 0) and
       (.windows_executable | type == "string" and length > 0) and
       (.executable | type == "string" and
         test("^[A-Za-z0-9_][A-Za-z0-9 ._-]{0,123}\\.[eE][xX][eE]$") and
         (contains("..") | not))' <<<"$selection" >/dev/null ||
  { echo "Selection lacks exact Bottles registration metadata" >&2; exit 1; }
bottle="$(jq -er '.bottle' <<<"$selection")"
program="$(jq -er '.registered_program' <<<"$selection")"
windows_executable="$(jq -er '.windows_executable' <<<"$selection")"
exe="$(jq -er '.executable' <<<"$selection")"

root_raw="$(bottles-cli --json info bottles-path)"
[[ "$root_raw" == /* && "$root_raw" != *$'\n'* && "$root_raw" != *$'\r'* ]] ||
  { echo "Invalid native Bottles root" >&2; exit 1; }
root="$(realpath -e -- "$root_raw")"
[[ "$(realpath -e -- "$chosen")" == "$chosen" && "$chosen" == "$root/"* ]] ||
  { echo "Selected game is outside canonical native Bottles root" >&2; exit 1; }
relative="${chosen#"$root"/}"
actual_bottle="${relative%%/*}"
[[ "$bottle" == "$actual_bottle" &&
   "${relative#"$actual_bottle"/}" == drive_c/* &&
   -f "$chosen/$exe" && ! -L "$chosen/$exe" ]] ||
  { echo "Selected file or bottle identity differs from reviewed selection" >&2; exit 1; }

# Ensure the selected native registration still resolves to the very file
# reviewed in the manifest, not merely another program with a matching name
# somewhere in the same bottle. The native manager independently checks
# case-collisions and symlink/drive containment at final preflight.
windows_slashes="${windows_executable//\\//}"
if [[ ! "$windows_slashes" =~ ^[cC]:/ ]]; then
  echo "Selected registered game is not on this bottle's C: drive" >&2
  exit 1
fi
windows_relative="${windows_slashes:3}"
selected_relative="${relative#"$actual_bottle"/}"
selected_relative="${selected_relative#drive_c/}/$exe"
if [[ "${windows_relative,,}" != "${selected_relative,,}" ]]; then
  echo "Registered executable path does not match selected game directory" >&2
  exit 1
fi

registered="$(bottles-cli --json programs -b "$bottle")"
jq -e --arg name "$program" --arg executable "$windows_executable" '
  type == "array" and
  ([.[] | select(.name == $name)] | length == 1) and
  ([.[] | select(.name == $name and .path == $executable)] | length == 1)
' <<<"$registered" >/dev/null ||
  { echo "Registered Bottles program changed or is ambiguous: $program" >&2; exit 1; }

if [[ "$renderer" == auto ]]; then
  command -v x86_64-w64-mingw32-objdump >/dev/null ||
    { echo "Cannot detect renderer without x86_64-w64-mingw32-objdump" >&2; exit 2; }
  imports="$(x86_64-w64-mingw32-objdump -p "$chosen/$exe")" ||
    { echo "Cannot inspect selected PE renderer imports" >&2; exit 1; }
  gl=false; vk=false; dx=false
  if grep -Eiq '^[[:space:]]*DLL Name:[[:space:]]*opengl32\.dll[[:space:]]*
  else
    echo "Renderer is ambiguous or dynamically loaded. Use --renderer opengl or --renderer vulkan after inspecting this game; no renderer was assumed." >&2
    exit 1
  fi
  echo "Renderer hint from selected PE static imports: $renderer (not in-game verification)." >&2
fi

args=(profile-launch --config "$source" --bottle "$bottle"
  --exe "$windows_executable" --program "$program"
  --profile "$profile" --offline-single-player
  --mangohud --managed-mangohud)
[[ "$timers" == false ]] || args+=(--experimental-four-timers)
[[ "$renderer" != opengl ]] || args+=(--mangohud-dlsym)
if [[ "$action" == check ]]; then args+=(--check); else args+=(--launch); fi

# All security and policy validation remains inside the existing native
# profile-launch implementation, including owned HUD and delivered profile.
exec "$manager" "${args[@]}"
 <<<"$imports"; then gl=true; fi
  if grep -Eiq '^[[:space:]]*DLL Name:[[:space:]]*vulkan-1\.dll[[:space:]]*
  else
    echo "Renderer is ambiguous or dynamically loaded. Use --renderer opengl or --renderer vulkan after inspecting this game; no renderer was assumed." >&2
    exit 1
  fi
  echo "Renderer hint from selected PE static imports: $renderer (not in-game verification)." >&2
fi

args=(profile-launch --config "$source" --bottle "$bottle"
  --exe "$windows_executable" --program "$program"
  --profile "$profile" --offline-single-player
  --mangohud --managed-mangohud)
[[ "$timers" == false ]] || args+=(--experimental-four-timers)
[[ "$renderer" != opengl ]] || args+=(--mangohud-dlsym)
if [[ "$action" == check ]]; then args+=(--check); else args+=(--launch); fi

# All security and policy validation remains inside the existing native
# profile-launch implementation, including owned HUD and delivered profile.
exec "$manager" "${args[@]}"
 <<<"$imports"; then vk=true; fi
  if grep -Eiq '^[[:space:]]*DLL Name:[[:space:]]*(d3d[0-9]+|dxgi|ddraw)\.dll[[:space:]]*
  else
    echo "Renderer is ambiguous or dynamically loaded. Use --renderer opengl or --renderer vulkan after inspecting this game; no renderer was assumed." >&2
    exit 1
  fi
  echo "Renderer hint from selected PE static imports: $renderer (not in-game verification)." >&2
fi

args=(profile-launch --config "$source" --bottle "$bottle"
  --exe "$windows_executable" --program "$program"
  --profile "$profile" --offline-single-player
  --mangohud --managed-mangohud)
[[ "$timers" == false ]] || args+=(--experimental-four-timers)
[[ "$renderer" != opengl ]] || args+=(--mangohud-dlsym)
if [[ "$action" == check ]]; then args+=(--check); else args+=(--launch); fi

# All security and policy validation remains inside the existing native
# profile-launch implementation, including owned HUD and delivered profile.
exec "$manager" "${args[@]}"
 <<<"$imports"; then dx=true; fi
  # A Direct3D import means the backend might be WineD3D or DXVK even if
  # OpenGL/Vulkan loader imports are also present. Never call that unique.
  if [[ "$gl" == true && "$vk" == false && "$dx" == false ]]; then
    renderer=opengl
  elif [[ "$vk" == true && "$gl" == false && "$dx" == false ]]; then
    renderer=vulkan
  else
    echo "Renderer is ambiguous or dynamically loaded. Use --renderer opengl or --renderer vulkan after inspecting this game; no renderer was assumed." >&2
    exit 1
  fi
  echo "Renderer hint from selected PE static imports: $renderer (not in-game verification)." >&2
fi

args=(profile-launch --config "$source" --bottle "$bottle"
  --exe "$windows_executable" --program "$program"
  --profile "$profile" --offline-single-player
  --mangohud --managed-mangohud)
[[ "$timers" == false ]] || args+=(--experimental-four-timers)
[[ "$renderer" != opengl ]] || args+=(--mangohud-dlsym)
if [[ "$action" == check ]]; then args+=(--check); else args+=(--launch); fi

# All security and policy validation remains inside the existing native
# profile-launch implementation, including owned HUD and delivered profile.
exec "$manager" "${args[@]}"
