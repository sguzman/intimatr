//! Platform-neutral edge detector between an explicitly focused game window
//! and the shared speed hotkey dispatcher. No keyboard or OS hooks live here.
//!
//! A host supplies focus and a snapshot of *only* its owned game's key state.
//! Initial held keys are seeded, not dispatched, on focus acquisition.

use std::collections::BTreeSet;

use crate::speed_hotkeys::{KeyEvent, Modifiers, SpeedHotkeyDispatcher};
use crate::speed_session::SpeedAction;

const CONTROL: u16 = 0x11;
const ALT: u16 = 0x12;
const SHIFT: u16 = 0x10;

#[derive(Debug, Clone, Default)]
pub struct InputSnapshot {
    /// This must be established independently of a mere matching executable
    /// name or guessed process ID. An unfocused snapshot never carries keys.
    pub focused: bool,
    pub down_keys: Vec<u16>,
}

#[derive(Debug)]
pub struct FocusedInputPump {
    dispatcher: SpeedHotkeyDispatcher,
    keys: Vec<u16>,
    previously_down: BTreeSet<u16>,
    focused: bool,
}

impl FocusedInputPump {
    pub fn new(dispatcher: SpeedHotkeyDispatcher) -> Self {
        let mut keys = dispatcher.watched_keys();
        keys.extend([CONTROL, ALT, SHIFT]);
        keys.sort_unstable();
        keys.dedup();
        Self {
            dispatcher,
            keys,
            previously_down: BTreeSet::new(),
            focused: false,
        }
    }

    /// The host polls exactly these virtual keys, not all keyboard input.
    #[must_use]
    pub fn watched_keys(&self) -> &[u16] {
        &self.keys
    }

    /// Returns a small, ordered batch of actions; host may submit each
    /// through the same shared speed session used by UI/RPC. A focus loss
    /// yields one reset, and focus gain never activates an already-held key.
    pub fn sample(&mut self, snapshot: InputSnapshot) -> Vec<SpeedAction> {
        if !snapshot.focused {
            if !self.focused {
                return Vec::new();
            }
            self.focused = false;
            self.previously_down.clear();
            return vec![self.dispatcher.on_focus_lost()];
        }

        let down: BTreeSet<u16> = snapshot
            .down_keys
            .into_iter()
            .filter(|key| self.keys.binary_search(key).is_ok())
            .collect();
        if !self.focused {
            self.focused = true;
            self.previously_down = down;
            self.dispatcher.on_focus_gained();
            return Vec::new();
        }

        let modifiers = Modifiers {
            ctrl: down.contains(&CONTROL),
            alt: down.contains(&ALT),
            shift: down.contains(&SHIFT),
        };
        let mut actions = Vec::new();
        // Apply releases first. A release must cancel temporary modulation
        // before another key press in the same observation activates a speed.
        for key in self.previously_down.difference(&down) {
            if let Some(action) = self.dispatcher.on_key(KeyEvent {
                virtual_key: *key,
                modifiers,
                pressed: false,
                repeat: false,
            }) {
                actions.push(action);
            }
        }
        // Modifiers are not actions themselves; their release was observed
        // above and can terminate a hold before a new chord is considered.
        for key in down.difference(&self.previously_down) {
            if let Some(action) = self.dispatcher.on_key(KeyEvent {
                virtual_key: *key,
                modifiers,
                pressed: true,
                repeat: false,
            }) {
                actions.push(action);
            }
        }
        self.previously_down = down;
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speed_config::SpeedSettings;
    use crate::speed_hotkeys::{HoldHotkey, SpeedHotkeyConfig};

    fn snapshot(focused: bool, keys: &[u16]) -> InputSnapshot {
        InputSnapshot {
            focused,
            down_keys: keys.to_vec(),
        }
    }

    fn pump() -> FocusedInputPump {
        let config = SpeedHotkeyConfig {
            increase: Some("Ctrl+F7".to_owned()),
            reset: Some("Alt+F8".to_owned()),
            hold: Some(HoldHotkey {
                chord: "Ctrl+H".to_owned(),
                multiplier: 2.0,
                duration_ms: 200,
            }),
            ..Default::default()
        };
        FocusedInputPump::new(config.compile(&SpeedSettings::default()).unwrap())
    }

    #[test]
    fn no_out_of_focus_or_preheld_activation() {
        let mut input = pump();
        assert!(input.sample(snapshot(false, &[CONTROL, 0x76])).is_empty());
        assert!(input.sample(snapshot(true, &[CONTROL, 0x76])).is_empty());
        assert!(input.sample(snapshot(true, &[CONTROL, 0x76])).is_empty());
        assert!(input.sample(snapshot(true, &[CONTROL])).is_empty());
        assert_eq!(
            input.sample(snapshot(true, &[CONTROL, 0x76])),
            vec![SpeedAction::Increase]
        );
        assert!(input.sample(snapshot(true, &[CONTROL, 0x76])).is_empty());
    }

    #[test]
    fn held_speed_releases_on_modifier_loss_and_focus_loss() {
        let mut input = pump();
        assert!(input.sample(snapshot(true, &[])).is_empty());
        assert_eq!(
            input.sample(snapshot(true, &[CONTROL, 0x48])),
            vec![SpeedAction::HoldStart {
                multiplier: 2.0,
                duration_ms: 200,
            }]
        );
        assert!(input.sample(snapshot(true, &[CONTROL, 0x48])).is_empty());
        assert_eq!(
            input.sample(snapshot(true, &[0x48])),
            vec![SpeedAction::HoldRelease]
        );
        assert!(input.sample(snapshot(true, &[])).is_empty());
        assert_eq!(
            input.sample(snapshot(true, &[CONTROL, 0x48])),
            vec![SpeedAction::HoldStart {
                multiplier: 2.0,
                duration_ms: 200,
            }]
        );
        assert_eq!(
            input.sample(snapshot(false, &[CONTROL, 0x48])),
            vec![SpeedAction::FocusLost]
        );
        assert!(input.sample(snapshot(false, &[])).is_empty());
        assert!(input.sample(snapshot(true, &[CONTROL, 0x48])).is_empty());
    }

    #[test]
    fn modifier_and_key_released_together_only_release_once() {
        let mut input = pump();
        input.sample(snapshot(true, &[]));
        input.sample(snapshot(true, &[CONTROL, 0x48]));
        assert_eq!(
            input.sample(snapshot(true, &[])),
            vec![SpeedAction::HoldRelease]
        );
    }

    #[test]
    fn focused_chords_use_one_shared_speed_session_with_continuous_clock() {
        use crate::speed_session::SpeedSession;

        let mut input = pump();
        let mut session = SpeedSession::new(&SpeedSettings::default(), 0, 1_000, 0, 0).unwrap();

        assert!(input.sample(snapshot(true, &[])).is_empty());
        for action in input.sample(snapshot(true, &[CONTROL, 0x76])) {
            session.apply(100, action).unwrap();
        }
        assert_eq!(session.multiplier(), 1.5);
        assert_eq!(session.sample(200).tick_count64, 250);
        assert!(input.sample(snapshot(true, &[CONTROL])).is_empty());

        for action in input.sample(snapshot(true, &[CONTROL, 0x48])) {
            session.apply(200, action).unwrap();
        }
        assert_eq!(session.multiplier(), 2.0);
        assert_eq!(session.sample(300).tick_count64, 450);

        for action in input.sample(snapshot(true, &[0x48])) {
            session.apply(300, action).unwrap();
        }
        assert_eq!(session.multiplier(), 1.5);
        assert_eq!(session.sample(400).tick_count64, 600);

        for action in input.sample(snapshot(false, &[])) {
            session.apply(400, action).unwrap();
        }
        assert_eq!(session.multiplier(), 1.0);
        assert_eq!(session.sample(500).tick_count64, 700);
    }

    #[test]
    fn reserved_keys_are_not_polled_and_one_shot_reset_is_stable() {
        let mut input = pump();
        assert!(!input.watched_keys().contains(&0x2d));
        assert!(!input.watched_keys().contains(&0x79));
        input.sample(snapshot(true, &[]));
        assert_eq!(
            input.sample(snapshot(true, &[ALT, 0x77])),
            vec![SpeedAction::Reset]
        );
        assert!(input.sample(snapshot(true, &[ALT, 0x77])).is_empty());
        assert!(input.sample(snapshot(true, &[ALT])).is_empty());
        assert_eq!(
            input.sample(snapshot(true, &[ALT, 0x77])),
            vec![SpeedAction::Reset]
        );
    }
}
