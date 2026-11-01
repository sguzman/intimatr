# Global speed profile — schema v1

Intimatr's proposed shared speed configuration is a **versioned TOML file**. The schema parser and validator are implemented in [src/speed_config.rs](../src/speed_config.rs), with an example in [config/speed-global.example.toml](../config/speed-global.example.toml).

**Important state distinction:** the schema and unit tests exist; **the Windows DLL does not yet resolve or apply this global file**, and per-bottle/per-game overrides are not implemented. A profile is not proof of a running speedhack.

## Example

```toml
version = 1

[speed]
enabled = false
initial_multiplier = 1.0
min_multiplier = 0.1
max_multiplier = 10.0
steps = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0]
```

| Key | Type | Default | Rule |
| --- | --- | --- | --- |
| `version` | integer | required | must be `1` |
| `speed.enabled` | boolean | `false` | disables automatic game-time modification by default |
| `speed.initial_multiplier` | positive finite float | `1.0` | inside min/max |
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

Keybinding grammar, increase/decrease, toggle/reset, and held-key speed presets are tracked under milestone 13. They are deliberately not accepted as undocumented keys by this v1 parser. The profile format should be extended in a version-aware compatible way when that implementation is ready.

The future loader manager will opt supported offline games into the DLL; a global profile is **not** blanket permission to inject into all Wine processes. Games with anti-cheat, protected execution or multiplayer features remain excluded.

## Verification

The parser's synthetic tests exercise safe defaults, custom ordered steps, schema version rejection, invalid bounds and values, unsupported keys, and TOML round trips. Test results must be confirmed in Linux or Windows CI at the exact commit before task INT-1201 can be marked verified.
