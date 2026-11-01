//! Portable speed preset/modulation state machine.
//!
//! This does not install hooks or dispatch keyboard events. Windows DLL input,
//! RPC and UI handlers will call these operations through the shared command
//! layer and apply `effective_multiplier` to the virtual clock.

use crate::speed_config::{SpeedProfileError, SpeedSettings};

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedControlState {
    steps: Vec<f64>,
    selected: f64,
    previous_nonunity: Option<f64>,
    bypassed: bool,
}

impl SpeedControlState {
    pub fn new(settings: &SpeedSettings) -> Result<Self, SpeedProfileError> {
        settings.validate()?;
        let initial = settings.initial_multiplier;
        let previous_nonunity = if initial != 1.0 {
            Some(initial)
        } else {
            settings
                .steps
                .iter()
                .copied()
                .find(|value| *value > 1.0)
                .or_else(|| settings.steps.iter().rev().copied().find(|v| *v < 1.0))
        };
        Ok(Self {
            steps: settings.steps.clone(),
            selected: initial,
            previous_nonunity,
            bypassed: !settings.enabled || initial == 1.0,
        })
    }

    #[must_use]
    pub fn effective_multiplier(&self) -> f64 {
        if self.bypassed { 1.0 } else { self.selected }
    }

    #[must_use]
    pub fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    /// Move to the next strictly faster configured preset.
    pub fn increase(&mut self) -> f64 {
        let current = self.effective_multiplier();
        if let Some(next) = self.steps.iter().copied().find(|step| *step > current) {
            self.activate(next);
        }
        self.effective_multiplier()
    }

    /// Move to the next strictly slower configured preset.
    pub fn decrease(&mut self) -> f64 {
        let current = self.effective_multiplier();
        if let Some(previous) = self.steps.iter().rev().copied().find(|step| *step < current) {
            self.activate(previous);
        }
        self.effective_multiplier()
    }

    /// Apply one exact configured preset; return false if it is not allowed.
    pub fn select_preset(&mut self, multiplier: f64) -> bool {
        if !multiplier.is_finite() || !self.steps.contains(&multiplier) {
            return false;
        }
        self.activate(multiplier);
        true
    }

    /// Temporarily bypass scaling, or restore the last non-1× selection.
    pub fn toggle(&mut self) -> f64 {
        if !self.bypassed {
            self.bypassed = true;
        } else if let Some(previous) = self.previous_nonunity {
            self.selected = previous;
            self.bypassed = false;
        }
        self.effective_multiplier()
    }

    /// Return to normal time without forgetting the last faster/slower preset.
    pub fn reset(&mut self) -> f64 {
        self.selected = 1.0;
        self.bypassed = true;
        self.effective_multiplier()
    }

    fn activate(&mut self, multiplier: f64) {
        self.selected = multiplier;
        self.bypassed = multiplier == 1.0;
        if multiplier != 1.0 {
            self.previous_nonunity = Some(multiplier);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpeedControlState;
    use crate::speed_config::SpeedSettings;

    #[test]
    fn steps_up_and_down_from_normal_speed() {
        let mut state = SpeedControlState::new(&SpeedSettings::default()).unwrap();
        assert_eq!(state.effective_multiplier(), 1.0);
        assert_eq!(state.increase(), 1.5);
        assert_eq!(state.increase(), 2.0);
        assert_eq!(state.decrease(), 1.5);
        assert_eq!(state.decrease(), 1.0);
        assert_eq!(state.decrease(), 0.75);
    }

    #[test]
    fn toggle_restores_last_nonunity_speed() {
        let mut state = SpeedControlState::new(&SpeedSettings::default()).unwrap();
        assert_eq!(state.toggle(), 1.5);
        assert_eq!(state.toggle(), 1.0);
        assert_eq!(state.toggle(), 1.5);
        assert_eq!(state.reset(), 1.0);
        assert_eq!(state.toggle(), 1.5);
    }

    #[test]
    fn starts_with_selected_configured_speed() {
        let settings = SpeedSettings {
            enabled: true,
            initial_multiplier: 2.0,
            ..SpeedSettings::default()
        };
        let mut state = SpeedControlState::new(&settings).unwrap();
        assert_eq!(state.effective_multiplier(), 2.0);
        assert_eq!(state.toggle(), 1.0);
        assert_eq!(state.toggle(), 2.0);
    }

    #[test]
    fn disabled_startup_is_safe_at_one_x() {
        let settings = SpeedSettings {
            enabled: false,
            initial_multiplier: 3.0,
            ..SpeedSettings::default()
        };
        let mut state = SpeedControlState::new(&settings).unwrap();
        assert_eq!(state.effective_multiplier(), 1.0);
        assert_eq!(state.toggle(), 3.0);
    }

    #[test]
    fn presets_reject_nonconfigured_or_invalid_speeds() {
        let mut state = SpeedControlState::new(&SpeedSettings::default()).unwrap();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 1.25] {
            assert!(!state.select_preset(bad));
            assert_eq!(state.effective_multiplier(), 1.0);
        }
        assert!(state.select_preset(0.5));
        assert_eq!(state.effective_multiplier(), 0.5);
    }

    #[test]
    fn steps_stop_at_boundaries() {
        let mut state = SpeedControlState::new(&SpeedSettings::default()).unwrap();
        assert!(state.select_preset(10.0));
        assert_eq!(state.increase(), 10.0);
        assert!(state.select_preset(0.25));
        assert_eq!(state.decrease(), 0.25);
    }
}
