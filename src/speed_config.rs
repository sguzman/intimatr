//! Versioned, platform-neutral global speed-profile configuration.
//!
//! Resolving this file through Wine and composing per-bottle/per-game overrides
//! are separate tasks. This type does not replace the legacy per-executable
//! scanner/debugger AppConfig.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CURRENT_SPEED_PROFILE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSpeedConfig {
    pub version: u32,
    #[serde(default)]
    pub speed: SpeedSettings,
}

impl SharedSpeedConfig {
    /// Parse a global TOML profile, preserving explicit errors for typos and
    /// incompatible schema versions.
    pub fn parse(input: &str) -> Result<Self, SpeedProfileError> {
        let parsed: Self = toml::from_str(input)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<(), SpeedProfileError> {
        if self.version != CURRENT_SPEED_PROFILE_VERSION {
            return Err(SpeedProfileError::Invalid(
                "unsupported speed profile version",
            ));
        }
        self.speed.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeedSettings {
    pub enabled: bool,
    pub initial_multiplier: f64,
    pub min_multiplier: f64,
    pub max_multiplier: f64,
    pub steps: Vec<f64>,
}

impl Default for SpeedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            initial_multiplier: 1.0,
            min_multiplier: 0.1,
            max_multiplier: 10.0,
            steps: vec![0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0],
        }
    }
}

impl SpeedSettings {
    pub fn validate(&self) -> Result<(), SpeedProfileError> {
        let bounds = [
            self.initial_multiplier,
            self.min_multiplier,
            self.max_multiplier,
        ];
        if bounds.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(SpeedProfileError::Invalid(
                "speed multipliers must be finite and positive",
            ));
        }
        if self.min_multiplier > 1.0 || self.max_multiplier < 1.0 {
            return Err(SpeedProfileError::Invalid(
                "speed range must include reset speed 1.0",
            ));
        }
        if self.initial_multiplier < self.min_multiplier
            || self.initial_multiplier > self.max_multiplier
        {
            return Err(SpeedProfileError::Invalid(
                "initial multiplier must lie within speed range",
            ));
        }
        if self.steps.is_empty() {
            return Err(SpeedProfileError::Invalid(
                "speed steps cannot be empty",
            ));
        }
        if self
            .steps
            .iter()
            .any(|v| !v.is_finite() || *v < self.min_multiplier || *v > self.max_multiplier)
        {
            return Err(SpeedProfileError::Invalid(
                "all speed steps must be finite and within speed range",
            ));
        }
        if self.steps.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(SpeedProfileError::Invalid(
                "speed steps must be strictly increasing",
            ));
        }
        if !self.steps.contains(&1.0) {
            return Err(SpeedProfileError::Invalid(
                "speed steps must include reset speed 1.0",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SpeedProfileError {
    #[error("invalid speed profile TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid speed profile: {0}")]
    Invalid(&'static str),
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_SPEED_PROFILE_VERSION, SharedSpeedConfig, SpeedSettings};

    #[test]
    fn parses_partial_profile_with_safe_defaults() {
        let profile = SharedSpeedConfig::parse("version = 1\n[speed]\nenabled = true\n")
            .expect("partial speed profile must be valid");
        assert_eq!(profile.version, CURRENT_SPEED_PROFILE_VERSION);
        assert!(profile.speed.enabled);
        assert_eq!(profile.speed.initial_multiplier, 1.0);
        assert_eq!(profile.speed.steps, SpeedSettings::default().steps);
    }

    #[test]
    fn rejects_incompatible_or_missing_schema_version() {
        for bad in ["version = 2", "[speed]\nenabled = true"] {
            assert!(SharedSpeedConfig::parse(bad).is_err());
        }
    }

    #[test]
    fn supports_custom_sorted_steps() {
        let profile = SharedSpeedConfig::parse(
            "version = 1\n[speed]\nenabled = true\ninitial_multiplier = 1.5\nmin_multiplier = 0.5\nmax_multiplier = 3.0\nsteps = [0.5, 1.0, 1.5, 2.0, 3.0]\n",
        )
        .expect("valid custom profile");
        assert_eq!(profile.speed.initial_multiplier, 1.5);
        assert_eq!(profile.speed.steps.len(), 5);
    }

    #[test]
    fn rejects_invalid_bounds_defaults_steps_and_unknown_keys() {
        let invalid = [
            "min_multiplier = 0.0",
            "min_multiplier = 2.0",
            "max_multiplier = 0.5",
            "initial_multiplier = 20.0",
            "steps = []",
            "steps = [1.0, 0.5]",
            "steps = [0.5, 0.5, 1.0]",
            "steps = [0.5, 2.0]",
            "steps = [0.5, 1.0, 11.0]",
            "initial_multiplier = nan",
            "max_multiplier = inf",
            "mispelled_option = true",
        ];
        for body in invalid {
            let profile = format!("version = 1\n[speed]\n{body}\n");
            assert!(
                SharedSpeedConfig::parse(&profile).is_err(),
                "expected profile to reject {body}"
            );
        }
    }

    #[test]
    fn config_round_trip_preserves_selection() {
        let original = SharedSpeedConfig {
            version: CURRENT_SPEED_PROFILE_VERSION,
            speed: SpeedSettings {
                enabled: true,
                initial_multiplier: 1.5,
                ..SpeedSettings::default()
            },
        };
        let encoded = toml::to_string(&original).expect("serialize profile");
        assert_eq!(
            SharedSpeedConfig::parse(&encoded).expect("deserialize profile"),
            original
        );
    }
}
