//! Conservative, portable window candidate policy for focused game hotkeys.
//! A process may host the game, its launcher, Intimatr tool UI and popups.
//! Never choose an arbitrary first match or capture from a foreign window.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    pub id: u64,
    pub current_process: bool,
    pub top_level: bool,
    pub visible: bool,
    /// Non-null Win32 owner HWND (not the process ID).
    pub owned: bool,
    pub tool_window: bool,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    NoEligibleWindow,
    AmbiguousWindows,
    EnumerationFailed,
}

impl WindowCandidate {
    #[must_use]
    pub fn eligible(&self) -> bool {
        self.id != 0
            && self.current_process
            && self.top_level
            && self.visible
            && !self.owned
            && !self.tool_window
            && !is_intimatr_tool_title(&self.title)
    }
}

/// Exclude Intimatr's in-process egui windows, which would otherwise compete
/// with the game's top-level HWND for keyboard focus.
fn is_intimatr_tool_title(title: &str) -> bool {
    title.starts_with("Intimatr — ") || title.starts_with("Intimatr Debugger — ")
}

/// Return a single unambiguous, owned, visible game window. If the process has
/// multiple plausible windows, input must remain disabled until a target can
/// be explicitly identified. Enumeration order is deliberately irrelevant.
pub fn choose_unique(candidates: &[WindowCandidate]) -> Result<u64, SelectionError> {
    let mut selected = None;
    for candidate in candidates.iter().filter(|window| window.eligible()) {
        if selected.replace(candidate.id).is_some() {
            return Err(SelectionError::AmbiguousWindows);
        }
    }
    selected.ok_or(SelectionError::NoEligibleWindow)
}

/// Preserve the identity of the one approved input window. A direct
/// switch from one live HWND to a different HWND must inject one synthetic
/// focus-loss sample before the new window's keyboard state is accepted.
/// This prevents a key held in a launcher/popup from becoming a speed action.
#[derive(Debug, Default)]
pub struct FocusedWindowEpoch {
    active_window: Option<u64>,
}

impl FocusedWindowEpoch {
    pub fn observe(
        &mut self,
        candidate: Option<(u64, crate::speed_input::InputSnapshot)>,
    ) -> crate::speed_input::InputSnapshot {
        let Some((window, snapshot)) = candidate else {
            self.active_window = None;
            return crate::speed_input::InputSnapshot::default();
        };
        if window == 0 || !snapshot.focused {
            self.active_window = None;
            return crate::speed_input::InputSnapshot::default();
        }
        let switched = self
            .active_window
            .is_some_and(|previous| previous != window);
        self.active_window = Some(window);
        if switched {
            crate::speed_input::InputSnapshot::default()
        } else {
            snapshot
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u64) -> WindowCandidate {
        WindowCandidate {
            id,
            current_process: true,
            top_level: true,
            visible: true,
            owned: false,
            tool_window: false,
            title: "Offline Game".to_owned(),
        }
    }

    #[test]
    fn target_window_switch_forces_a_focus_loss_before_accepting_new_keys() {
        use crate::speed_input::InputSnapshot;

        let focused = |keys: &[u16]| InputSnapshot {
            focused: true,
            down_keys: keys.to_vec(),
        };
        let mut epoch = FocusedWindowEpoch::default();
        assert!(epoch.observe(Some((10, focused(&[0x11])))).focused);
        assert!(epoch.observe(Some((10, focused(&[0x11, 0x76])))).focused);
        let switched = epoch.observe(Some((20, focused(&[0x11, 0x76]))));
        assert!(!switched.focused);
        assert!(switched.down_keys.is_empty());
        assert!(epoch.observe(Some((20, focused(&[0x11, 0x76])))).focused);
        assert!(!epoch.observe(None).focused);
        assert!(epoch.observe(Some((20, focused(&[0x76])))).focused);
        assert!(!epoch.observe(Some((0, focused(&[0x76])))).focused);
    }

    #[test]
    fn chooses_only_unambiguous_owned_game_window() {
        assert_eq!(choose_unique(&[candidate(33)]), Ok(33));
        assert_eq!(
            choose_unique(&[candidate(33), candidate(44)]),
            Err(SelectionError::AmbiguousWindows)
        );
        assert_eq!(
            choose_unique(&[candidate(44), candidate(33)]),
            Err(SelectionError::AmbiguousWindows)
        );
        assert_eq!(choose_unique(&[]), Err(SelectionError::NoEligibleWindow));
    }

    #[test]
    fn excludes_tool_and_foreign_windows_even_if_foreground() {
        let mut windows = vec![
            candidate(33),
            candidate(44),
            candidate(55),
            candidate(66),
            candidate(77),
        ];
        windows[1].current_process = false;
        windows[2].tool_window = true;
        windows[3].owned = true;
        windows[4].visible = false;
        assert_eq!(choose_unique(&windows), Ok(33));
        windows[0].top_level = false;
        assert_eq!(
            choose_unique(&windows),
            Err(SelectionError::NoEligibleWindow)
        );
    }

    #[test]
    fn refuses_intimatr_frontends_and_invalid_handles() {
        let mut windows = vec![candidate(0), candidate(22), candidate(33)];
        windows[1].title = "Intimatr — Game.exe".into();
        windows[2].title = "Intimatr Debugger — Game.exe".into();
        assert_eq!(
            choose_unique(&windows),
            Err(SelectionError::NoEligibleWindow)
        );
        windows[2].title = "Actual Game".into();
        assert_eq!(choose_unique(&windows), Ok(33));
    }
}
