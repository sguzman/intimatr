# Global speed profile — schema v1

Intimatr's proposed shared speed configuration is a **versioned TOML file**. The schema parser and validator are implemented in [src/speed_config.rs](../src/speed_config.rs), with an example in [config/speed-global.example.toml](../config/speed-global.example.toml).

**Important state distinction:** the schema and unit tests exist; **the Windows DLL does not yet resolve or apply this global file**, and per-bottle/per-game overrides are not implemented. A profile is not proof of a running speedhack.

## Example

```toml
version = 1

[speed]
enabled = false
initial_multiplier = 1.0
startup_duration_ms = 0
min_multiplier = 0.1
max_multiplier = 10.0
steps = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0]
```

| Key | Type | Default | Rule |
| --- | --- | --- | --- |
| `version` | integer | required | must be `1` |
| `speed.enabled` | boolean | `false` | disables automatic game-time modification by default |
| `speed.initial_multiplier` | positive finite float | `1.0` | inside min/max |
| `speed.startup_duration_ms` | integer | `0` | optional startup reset after 1–60,000 ms; `0` disables the timer |
| `speed.min_multiplier` | positive finite float | `0.1` | at most `1.0` |
| `speed.max_multiplier` | positive finite float | `10.0` | at least `1.0` |
| `speed.steps` | array of positive finite floats | `[0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0]` | non-empty, strictly increasing, within bounds, contains `1.0` |

Unknown keys are rejected to expose misspellings. Missing optional `[speed]` keys use safe defaults, but the root version is required. Invalid files produce errors and cannot silently change the engine's effective speed. The schema's type and validation errors are part of the upcoming native CLI's reporting surface.

## Configuration precedence (planned)

1. Built-in safe defaults, initially disabled and `1.0×`.
2. Linux global user config, proposed at `~/.config/intimatr/intimatr.toml`.
3. Per-bottle filter and override.
4. Per-executable settings (with unambiguous bottle/executable identity).
5. Ephemeral live speed changes from hotkeys, UI or RPC.

The runtime must resolve Linux host paths to Wine-visible paths without assuming that `~` is meaningful inside the Windows process. Avoid copying profiles into every game folder; store the shared file once and distribute only the minimum loader/bootstrap state required. Changes to the original `config/<ExecutableName>.toml` for scanner/debugger configuration must remain backward compatible.

## Keybindings and optional features

Keybinding grammar, increase/decrease, toggle/reset, and held-key speed presets are tracked under milestone 13. They are deliberately not accepted as undocumented keys by this v1 parser. The portable speed session now exposes a provisional bounded `HoldStart { multiplier, duration_ms }` action accepting an existing configured step and a duration from 1 to 60,000 ms, plus `HoldRelease` and `FocusLost`. Holds automatically restore the previous selection at their deadline; focus loss resets to 1×. A configured `startup_duration_ms` resets an enabled non-1× initial speed after a bounded real-QPC interval; zero disables this startup timer, and explicit persistent speed changes supersede it. Startup expiry also cancels any overlapping hold. These are core APIs, **not yet keyboard bindings or a running Wine-game implementation**. The profile format should be extended in a version-aware compatible way when input integration is ready.

The future loader manager will opt supported offline games into the DLL; a global profile is **not** blanket permission to inject into all Wine processes. Games with anti-cheat, protected execution or multiplayer features remain excluded.

## Verification

The parser's synthetic tests exercise safe defaults, custom ordered steps, schema version rejection, invalid bounds and values, unsupported keys, and TOML round trips. Test results must be confirmed in Linux or Windows CI at the exact commit before task INT-1201 can be marked verified.

## Bottle and executable speed rules (portable matching implemented)

The shared version-1 TOML can now contain optional `[[bottles]]` and `[[games]]` overrides. The **portable parser, precedence resolver, identity checks, and unit tests** are implemented in [src/speed_policy.rs](../src/speed_policy.rs). These are not yet connected to live Bottles discovery, DLL startup or automatic game enrollment.

```toml
version = 1

[speed]
enabled = false

[[bottles]]
name = "Games"
[bottles.speed]
enabled = true
initial_multiplier = 1.5

[[games]]
bottle = "Games"
path = 'C:\Games\SinglePlayer\Game.exe'
[games.speed]
initial_multiplier = 2.0

[[games]]
bottle = "Online"
path = 'C:\Games\NetworkGame.exe'
exclude = true
```

Resolution order is **global → matching bottle → matching full executable path**. The per-rule `[bottles.speed]` and `[games.speed]` tables allow optional `enabled`, `initial_multiplier`, `startup_duration_ms`, `min_multiplier`, `max_multiplier`, and `steps` keys. Validation applies after merging so an out-of-range final profile fails instead of silently clamping. An `exclude = true` in either the bottle or game layer always wins over a more specific `enabled = true`.

Bottle names are matched with trimmed ASCII-insensitive comparisons. Executable rules require a drive-qualified absolute Windows path, normalize ASCII case and slash separators, and reject `.`, `..`, UNC paths and basename-only rules. The complete bottle-plus-path identity distinguishes same-named `.exe` files in different game directories or bottles. Duplicate normalized rules, unknown fields and invalid effective speeds fail profile loading. Windows Unicode case-folding, mapped-drive aliases, filesystem links and native Bottles path identity remain future compatibility gates.

**Important:** a speed rule is data describing a hypothetical target's speed; it does not grant process attachment or enroll all matching games. Deployment remains a separate opt-in flow. INT-1202 requires validating actual native Bottles inventory and path identities before its weighted completion is credited.

### Read-only preview from Linux

The native manager can inspect the resolved speed for a specifically named bottle and complete Windows executable path, **without launching Bottles, loading a DLL or modifying files**:

```fish
cargo run --locked --bin intimatr-bottles -- profile-check --config config/speed-global.example.toml --bottle Games --exe 'C:\Games\SinglePlayer\Game.exe'
```

It prints `operation = read_only_profile_resolution` with the matched bottle/game flags, exclusion status and effective speed settings. The flags `dll_loaded = false` and `attachment_attempted = false` are deliberate: this command only resolves TOML, and cannot claim the target process exists. This CLI is a profile-preview tool and not a substitute for actual Bottles identity and Wine path compatibility testing.

## Focused-window speed hotkeys (portable configuration stage)

Speed bindings are optional and disabled by omission. They are parsed and validated in the shared v1 TOML profile, but **not yet delivered from an actual game window**, so they do not currently control a game. This example describes a proposed focused-window binding set, not a Wayland-global grab:

```toml
[hotkeys]
increase = "Ctrl+F7"
decrease = "Ctrl+F6"
toggle = "Alt+F7"
reset = "Alt+F6"

[hotkeys.hold]
chord = "Ctrl+H"
multiplier = 2.0
duration_ms = 500
```

Valid modifiers are Ctrl/Control, Alt and Shift, in any order. Keys include letters A–Z, digits 0–9, F1–F24, navigation keys, Tab, Space, Enter, Backspace and Escape. The parser rejects duplicate modifiers, Win/Super shortcuts, ambiguous chords, duplicate action bindings, and all chords using **F10 or Insert** because the existing Intimatr UI polls those two keys independently of modifiers. A hold multiplier must be one of the final resolved profile's configured speed steps, including per-bottle and per-game overrides. Duration must be 1–60,000 ms. Keydown repeat is suppressed, and hold release works even if the user released a modifier first. Focus-loss events reset the session to 1×; real-game event integration remains a separate milestone acceptance gate.
