//! Portable, opt-in speed keybindings for a focused Wine game window.
//! This module parses configuration and dispatches *supplied* key events.
//! It does not install global shortcuts, poll keyboard state or load a DLL.
//! The Windows frontend must supply accurate focus and modifier events.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{speed_config::SpeedSettings, speed_session::SpeedAction};

/// No defaults intentionally intercept game input. Users opt in per action.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeedHotkeyConfig {
    pub increase: Option<String>,
    pub decrease: Option<String>,
    pub toggle: Option<String>,
    pub reset: Option<String>,
    pub hold: Option<HoldHotkey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldHotkey {
    pub chord: String,
    pub multiplier: f64,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    fn includes(self, expected: Self) -> bool {
        (!expected.ctrl || self.ctrl)
            && (!expected.alt || self.alt)
            && (!expected.shift || self.shift)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Windows virtual key code; does not encode a keyboard layout character.
    pub key: u16,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub virtual_key: u16,
    pub modifiers: Modifiers,
    pub pressed: bool,
    pub repeat: bool,
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Increase,
    Decrease,
    Toggle,
    Reset,
    Hold { multiplier: f64, duration_ms: u32 },
}

impl Action {
    fn speed_action(self) -> SpeedAction {
        match self {
            Self::Increase => SpeedAction::Increase,
            Self::Decrease => SpeedAction::Decrease,
            Self::Toggle => SpeedAction::Toggle,
            Self::Reset => SpeedAction::Reset,
            Self::Hold {
                multiplier,
                duration_ms,
            } => SpeedAction::HoldStart {
                multiplier,
                duration_ms,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpeedHotkeyDispatcher {
    bindings: Vec<(KeyChord, Action)>,
    pressed: HashSet<u16>,
    held: Option<KeyChord>,
    focused: bool,
}

impl SpeedHotkeyConfig {
    /// Validate all keys, avoid ambiguous action duplication, and prevent
    /// collision with Intimatr's legacy raw-key Insert/F10 UI toggles.
    pub fn compile(&self, speed: &SpeedSettings) -> Result<SpeedHotkeyDispatcher, &'static str> {
        let mut bindings = Vec::new();
        let mut seen = HashSet::new();
        for (raw, action) in [
            (self.increase.as_deref(), Action::Increase),
            (self.decrease.as_deref(), Action::Decrease),
            (self.toggle.as_deref(), Action::Toggle),
            (self.reset.as_deref(), Action::Reset),
        ] {
            if let Some(raw) = raw {
                add_binding(&mut bindings, &mut seen, raw, action)?;
            }
        }
        if let Some(ref hold) = self.hold {
            if !hold.multiplier.is_finite() || !speed.steps.contains(&hold.multiplier) {
                return Err("held hotkey multiplier must be a configured speed step");
            }
            if !(1..=60_000).contains(&hold.duration_ms) {
                return Err("held hotkey duration must be between 1 and 60000 ms");
            }
            add_binding(
                &mut bindings,
                &mut seen,
                &hold.chord,
                Action::Hold {
                    multiplier: hold.multiplier,
                    duration_ms: hold.duration_ms,
                },
            )?;
        }
        Ok(SpeedHotkeyDispatcher {
            bindings,
            pressed: HashSet::new(),
            held: None,
            focused: false,
        })
    }
}

fn add_binding(
    bindings: &mut Vec<(KeyChord, Action)>,
    seen: &mut HashSet<KeyChord>,
    raw: &str,
    action: Action,
) -> Result<(), &'static str> {
    let chord = parse_chord(raw)?;
    // Existing UI polls F10/Insert independently of modifier state.
    if chord.key == 0x79 || chord.key == 0x2d {
        return Err("F10 and Insert are reserved by the existing Intimatr UI");
    }
    if !seen.insert(chord) {
        return Err("duplicate speed hotkey chord");
    }
    bindings.push((chord, action));
    Ok(())
}

/// Strict ASCII grammar: Ctrl+Alt+Shift+KEY, in any modifier order.
/// KEY: A-Z, 0-9, F1-F24, navigation, Tab, Space, Enter or Escape.
/// Super/Win is intentionally excluded: compositor shortcuts under Wayland
/// are neither reliable nor owned by a Windows process.
pub fn parse_chord(text: &str) -> Result<KeyChord, &'static str> {
    if text.is_empty() || text.len() > 80 || !text.is_ascii() {
        return Err("invalid hotkey chord length or characters");
    }
    let tokens: Vec<_> = text.split('+').map(str::trim).collect();
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return Err("hotkey chord has an empty key or modifier");
    }
    let mut modifiers = Modifiers::default();
    for modifier in &tokens[..tokens.len() - 1] {
        match modifier.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" if !modifiers.ctrl => modifiers.ctrl = true,
            "ALT" if !modifiers.alt => modifiers.alt = true,
            "SHIFT" if !modifiers.shift => modifiers.shift = true,
            _ => return Err("unsupported or duplicate hotkey modifier"),
        }
    }
    let key = parse_key(tokens[tokens.len() - 1])?;
    Ok(KeyChord { key, modifiers })
}

fn parse_key(raw: &str) -> Result<u16, &'static str> {
    let key = raw.to_ascii_uppercase();
    if key.len() == 1 {
        let ch = key.as_bytes()[0];
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return Ok(u16::from(ch));
        }
    }
    if let Some(number) = key.strip_prefix('F')
        && let Ok(n) = number.parse::<u16>()
        && (1..=24).contains(&n)
    {
        return Ok(0x70 + n - 1);
    }
    match key.as_str() {
        "BACKSPACE" => Ok(0x08),
        "TAB" => Ok(0x09),
        "ENTER" | "RETURN" => Ok(0x0d),
        "ESC" | "ESCAPE" => Ok(0x1b),
        "SPACE" => Ok(0x20),
        "PAGEUP" => Ok(0x21),
        "PAGEDOWN" => Ok(0x22),
        "END" => Ok(0x23),
        "HOME" => Ok(0x24),
        "LEFT" => Ok(0x25),
        "UP" => Ok(0x26),
        "RIGHT" => Ok(0x27),
        "DOWN" => Ok(0x28),
        "INSERT" => Ok(0x2d),
        "DELETE" => Ok(0x2e),
        _ => Err("unsupported speed hotkey key"),
    }
}

impl SpeedHotkeyDispatcher {
    /// Exact virtual keys needed for the selected actions. The input pump
    /// adds modifier VKs separately, even if no chord uses them.
    #[must_use]
    pub fn watched_keys(&self) -> Vec<u16> {
        let mut keys: Vec<u16> = self.bindings.iter().map(|(chord, _)| chord.key).collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// The event stream remains disabled until the host explicitly reports
    /// focus. The host must stop feeding unrelated window events.
    pub fn on_focus_gained(&mut self) {
        if !self.focused {
            self.focused = true;
            self.pressed.clear();
        }
    }

    /// The frontend calls this only for events from its *focused* game window.
    /// Repeated keydown events cannot change speed more than once per press.
    /// Releases for a held chord are recognized even when modifiers changed.
    pub fn on_key(&mut self, event: KeyEvent) -> Option<SpeedAction> {
        if !self.focused {
            return None;
        }
        if let Some(held) = self.held
            && ((!event.pressed && event.virtual_key == held.key)
                || !event.modifiers.includes(held.modifiers))
        {
            self.held = None;
            self.pressed.remove(&held.key);
            // A new keydown in the same event must not also trigger a
            // second command; callers may deliver the next event normally.
            return Some(SpeedAction::HoldRelease);
        }

        if !event.pressed {
            self.pressed.remove(&event.virtual_key);
            return None;
        }
        if event.repeat || !self.pressed.insert(event.virtual_key) {
            return None;
        }
        let pressed = KeyChord {
            key: event.virtual_key,
            modifiers: event.modifiers,
        };
        for &(chord, action) in &self.bindings {
            if chord == pressed {
                if matches!(action, Action::Hold { .. }) {
                    self.held = Some(chord);
                }
                return Some(action.speed_action());
            }
        }
        None
    }

    /// The host must call this when the game loses focus or the input
    /// stream is interrupted; it must stop sending keys until focus returns.
    pub fn on_focus_lost(&mut self) -> SpeedAction {
        self.focused = false;
        self.pressed.clear();
        self.held = None;
        SpeedAction::FocusLost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(key: u16, modifiers: Modifiers, pressed: bool, repeat: bool) -> KeyEvent {
        KeyEvent {
            virtual_key: key,
            modifiers,
            pressed,
            repeat,
        }
    }

    #[test]
    fn canonical_modifier_order_and_reject_ambiguous_grammar() {
        assert_eq!(
            parse_chord("Shift+Ctrl+f7").unwrap(),
            parse_chord("control + shift + F7").unwrap()
        );
        assert_eq!(parse_chord("F24").unwrap().key, 0x87);
        for invalid in [
            "",
            "+A",
            "A+",
            "Ctrl++A",
            "Ctrl+Ctrl+A",
            "Meta+A",
            "Win+G",
            "Ctrl+Ctrl",
            "Ctrl+F25",
            "F0",
            "Ctrl+€",
        ] {
            assert!(parse_chord(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn reserved_ui_keys_and_duplicate_aliases_fail_closed() {
        let speed = SpeedSettings::default();
        for key in ["Insert", "Shift+Insert", "F10", "Ctrl+F10"] {
            let config = SpeedHotkeyConfig {
                toggle: Some(key.to_owned()),
                ..Default::default()
            };
            assert!(config.compile(&speed).is_err(), "{key}");
        }
        let duplicate = SpeedHotkeyConfig {
            increase: Some("Ctrl+Shift+F7".into()),
            decrease: Some("Shift+Control+F7".into()),
            ..Default::default()
        };
        assert!(duplicate.compile(&speed).is_err());
    }

    #[test]
    fn hold_must_use_allowed_step_and_bounded_duration() {
        let speed = SpeedSettings::default();
        for (multiplier, duration_ms) in
            [(1.25, 200), (f64::INFINITY, 200), (2.0, 0), (2.0, 60_001)]
        {
            let config = SpeedHotkeyConfig {
                hold: Some(HoldHotkey {
                    chord: "Alt+F7".into(),
                    multiplier,
                    duration_ms,
                }),
                ..Default::default()
            };
            assert!(config.compile(&speed).is_err());
        }
    }

    #[test]
    fn one_shot_keys_fire_once_per_down_and_ignore_repeats() {
        let config = SpeedHotkeyConfig {
            increase: Some("Ctrl+F7".into()),
            ..Default::default()
        };
        let mut keys = config.compile(&SpeedSettings::default()).unwrap();
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(keys.on_key(event(0x76, ctrl, true, false)), None);
        keys.on_focus_gained();
        assert_eq!(
            keys.on_key(event(0x76, ctrl, true, false)),
            Some(SpeedAction::Increase)
        );
        assert_eq!(keys.on_key(event(0x76, ctrl, true, true)), None);
        assert_eq!(keys.on_key(event(0x76, ctrl, true, false)), None);
        assert_eq!(keys.on_key(event(0x76, ctrl, false, false)), None);
        assert_eq!(
            keys.on_key(event(0x76, ctrl, true, false)),
            Some(SpeedAction::Increase)
        );
    }

    #[test]
    fn hold_release_survives_modifier_release_and_focus_loss() {
        let config = SpeedHotkeyConfig {
            hold: Some(HoldHotkey {
                chord: "Ctrl+H".into(),
                multiplier: 2.0,
                duration_ms: 150,
            }),
            ..Default::default()
        };
        let mut keys = config.compile(&SpeedSettings::default()).unwrap();
        keys.on_focus_gained();
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let none = Modifiers::default();
        assert_eq!(
            keys.on_key(event(0x48, ctrl, true, false)),
            Some(SpeedAction::HoldStart {
                multiplier: 2.0,
                duration_ms: 150
            })
        );
        assert_eq!(keys.on_key(event(0x48, ctrl, true, true)), None);
        assert_eq!(
            keys.on_key(event(0x11, none, false, false)),
            Some(SpeedAction::HoldRelease)
        );
        assert_eq!(keys.on_key(event(0x48, none, false, false)), None);
        assert_eq!(
            keys.on_key(event(0x48, ctrl, true, false)),
            Some(SpeedAction::HoldStart {
                multiplier: 2.0,
                duration_ms: 150
            })
        );
        assert_eq!(keys.on_focus_lost(), SpeedAction::FocusLost);
        assert_eq!(keys.on_key(event(0x48, none, false, false)), None);
        // A stale event stream must not reactivate a hold until the host
        // reports that the intended game window has regained focus.
        assert_eq!(keys.on_key(event(0x48, ctrl, true, false)), None);
        keys.on_focus_gained();
        assert_eq!(
            keys.on_key(event(0x48, ctrl, true, false)),
            Some(SpeedAction::HoldStart {
                multiplier: 2.0,
                duration_ms: 150
            })
        );
    }

    #[test]
    fn no_default_binding_intercepts_any_game_key() {
        let mut keys = SpeedHotkeyConfig::default()
            .compile(&SpeedSettings::default())
            .unwrap();
        keys.on_focus_gained();
        assert_eq!(
            keys.on_key(event(0x41, Modifiers::default(), true, false)),
            None
        );
        assert_eq!(keys.on_focus_lost(), SpeedAction::FocusLost);
    }
}
