//! Explicitly preview-only egui controls, backed by the same CommandExecutor
//! used by RPC and focused game hotkeys. No timer hook or local speed state
//! is installed by this view.
use eframe::egui;

use crate::speed_commands::{SpeedPreviewStatus, SpeedRequest};

#[derive(Default)]
pub(super) struct SpeedView {
    status: Option<SpeedPreviewStatus>,
    preset_input: String,
    validation_error: Option<String>,
}

impl SpeedView {
    pub(super) fn update(&mut self, status: SpeedPreviewStatus) {
        self.status = Some(status);
        self.validation_error = None;
    }

    pub(super) fn error(&mut self, error: String) {
        self.validation_error = Some(error);
    }

    pub(super) fn render(&mut self, ui: &mut egui::Ui, pending: bool) -> Option<SpeedRequest> {
        ui.heading("Speed control");
        ui.label(
            "Preview only. These controls change the shared command state; \
             they do not currently accelerate the game.",
        );

        let Some(status) = self.status.as_ref() else {
            ui.label("Speed status has not been received yet.");
            if ui
                .add_enabled(!pending, egui::Button::new("Refresh status"))
                .clicked()
            {
                return Some(SpeedRequest::Status);
            }
            return None;
        };

        let configured = status.configured;
        ui.horizontal(|ui| {
            ui.label("Shared speed profile:");
            ui.strong(if configured {
                "Configured"
            } else {
                "Not configured"
            });
        });
        if let Some(profile) = status.profile.as_deref() {
            ui.label(format!("Source: {profile}"));
        }
        ui.horizontal(|ui| {
            ui.label("Preview multiplier:");
            ui.strong(format!("{:.2}x", status.preview_multiplier));
        });
        ui.label(format!(
            "Game window focus: {}",
            if status.focused {
                "Focused"
            } else {
                "Unfocused"
            }
        ));
        if let Some(action) = &status.last_action {
            ui.label(format!("Last preview action: {action}"));
        }
        let applied = status.clock_applied_to_game;
        ui.horizontal(|ui| {
            ui.label("Game timing hook:");
            ui.strong(if applied {
                "Reported active"
            } else {
                "Not applied to game"
            });
        });
        if !applied {
            ui.label("A changed preview multiplier does not demonstrate changed game speed.");
        }
        if !configured {
            ui.label(
                "Set INTIMATR_SPEED_PROFILE to an absolute Windows-visible path to an enabled, \
                 validated global speed profile before starting the DLL.",
            );
        }
        if let Some(error) = &self.validation_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        let mut request = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!pending, egui::Button::new("Refresh"))
                .clicked()
            {
                request = Some(SpeedRequest::Status);
            }
            ui.add_enabled_ui(configured && !pending, |ui| {
                if ui.button("Slower").clicked() {
                    request = Some(SpeedRequest::Decrease);
                }
                if ui.button("Faster").clicked() {
                    request = Some(SpeedRequest::Increase);
                }
                if ui.button("Toggle 1x").clicked() {
                    request = Some(SpeedRequest::Toggle);
                }
                if ui.button("Reset 1x").clicked() {
                    request = Some(SpeedRequest::Reset);
                }
            });
        });
        ui.add_enabled_ui(configured && !pending, |ui| {
            ui.horizontal(|ui| {
                ui.label("Preset:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.preset_input)
                        .desired_width(90.0)
                        .hint_text("2.0"),
                );
                if ui.button("Select").clicked() {
                    match parse_preset(&self.preset_input) {
                        Ok(multiplier) => {
                            self.validation_error = None;
                            request = Some(SpeedRequest::SelectPreset { multiplier });
                        }
                        Err(error) => self.validation_error = Some(error.to_owned()),
                    }
                }
            });
        });
        if pending {
            ui.spinner();
        }
        request
    }
}

fn parse_preset(input: &str) -> Result<f64, &'static str> {
    let multiplier: f64 = input
        .trim()
        .parse()
        .map_err(|_| "Preset must be a finite positive number.")?;
    if !multiplier.is_finite() || multiplier <= 0.0 {
        return Err("Preset must be a finite positive number.");
    }
    Ok(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_preset_validation_rejects_nonpositive_and_nonfinite_values() {
        for input in ["", "0", "-1", "NaN", "inf", "abc"] {
            assert!(parse_preset(input).is_err(), "accepted {input}");
        }
        assert_eq!(parse_preset(" 2.5 ").unwrap(), 2.5);
    }

    #[test]
    fn received_speed_status_is_not_misrepresented_as_applied_game_time() {
        let mut view = SpeedView::default();
        view.error("old error".into());
        view.update(SpeedPreviewStatus {
            configured: true,
            profile: Some("test".to_owned()),
            preview_multiplier: 2.0,
            focused: false,
            last_action: None,
            clock_applied_to_game: false,
        });
        assert_eq!(view.status.as_ref().unwrap().preview_multiplier, 2.0);
        assert!(!view.status.as_ref().unwrap().clock_applied_to_game);
        assert!(view.validation_error.is_none());
    }
}
