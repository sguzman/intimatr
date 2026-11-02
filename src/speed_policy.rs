//! Strict, deterministic global → bottle → full-executable speed resolution.
//!
//! This is a platform-neutral *profile* resolver, not authorization to load
//! DLLs into programs. Runtime enrollment is a separate opt-in workflow.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::speed_config::{SharedSpeedConfig, SpeedProfileError, SpeedSettings};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeedOverride {
    pub enabled: Option<bool>,
    pub initial_multiplier: Option<f64>,
    pub startup_duration_ms: Option<u32>,
    pub min_multiplier: Option<f64>,
    pub max_multiplier: Option<f64>,
    pub steps: Option<Vec<f64>>,
}

impl SpeedOverride {
    fn apply_to(&self, settings: &mut SpeedSettings) {
        if let Some(value) = self.enabled {
            settings.enabled = value;
        }
        if let Some(value) = self.initial_multiplier {
            settings.initial_multiplier = value;
        }
        if let Some(value) = self.startup_duration_ms {
            settings.startup_duration_ms = value;
        }
        if let Some(value) = self.min_multiplier {
            settings.min_multiplier = value;
        }
        if let Some(value) = self.max_multiplier {
            settings.max_multiplier = value;
        }
        if let Some(ref value) = self.steps {
            settings.steps.clone_from(value);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BottleSpeedRule {
    pub name: String,
    #[serde(default)]
    pub exclude: bool,
    #[serde(default)]
    pub speed: SpeedOverride,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameSpeedRule {
    pub bottle: String,
    /// Absolute drive-qualified Windows path, not just an executable basename.
    pub path: String,
    #[serde(default)]
    pub exclude: bool,
    #[serde(default)]
    pub speed: SpeedOverride,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedSpeed {
    pub settings: SpeedSettings,
    pub excluded: bool,
    pub matched_bottle: bool,
    pub matched_game: bool,
}

fn bottle_key(name: &str) -> Result<String, SpeedProfileError> {
    let name = name.trim();
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(SpeedProfileError::Invalid("invalid bottle identity"));
    }
    Ok(name.to_ascii_lowercase())
}

pub(crate) fn game_path_key(path: &str) -> Result<String, SpeedProfileError> {
    let text = path.trim();
    let mut chars = text.chars();
    let drive = chars
        .next()
        .ok_or(SpeedProfileError::Invalid("empty Windows executable path"))?;
    if !drive.is_ascii_alphabetic() || chars.next() != Some(':') {
        return Err(SpeedProfileError::Invalid(
            "executable identity must be an absolute drive-qualified Windows path",
        ));
    }
    let remaining: String = chars.collect();
    let remaining = remaining.replace('\\', "/");
    if !remaining.starts_with('/') {
        return Err(SpeedProfileError::Invalid(
            "executable identity must be an absolute drive-qualified Windows path",
        ));
    }
    let mut components = Vec::new();
    for part in remaining.split('/').filter(|segment| !segment.is_empty()) {
        if part == "." || part == ".." || part.chars().any(char::is_control) {
            return Err(SpeedProfileError::Invalid(
                "relative segments are not allowed in executable identity",
            ));
        }
        components.push(part);
    }
    if components.is_empty() {
        return Err(SpeedProfileError::Invalid(
            "executable path lacks a filename",
        ));
    }
    Ok(format!(
        "{}:/{}",
        drive.to_ascii_lowercase(),
        components.join("/").to_ascii_lowercase()
    ))
}

/// Validate uniqueness and every effective override chain, even for rules
/// that do not currently match any program in the user's library.
pub fn validate_overrides(profile: &SharedSpeedConfig) -> Result<(), SpeedProfileError> {
    let mut bottles = HashSet::new();
    for rule in &profile.bottles {
        let identity = bottle_key(&rule.name)?;
        if !bottles.insert(identity) {
            return Err(SpeedProfileError::Invalid("duplicate bottle override"));
        }
        let mut effective = profile.speed.clone();
        rule.speed.apply_to(&mut effective);
        effective.validate()?;
    }

    let mut games = HashSet::new();
    for rule in &profile.games {
        let bottle = bottle_key(&rule.bottle)?;
        let path = game_path_key(&rule.path)?;
        if !games.insert((bottle.clone(), path)) {
            return Err(SpeedProfileError::Invalid("duplicate executable override"));
        }
        let mut effective = profile.speed.clone();
        if let Some(bottle_rule) = profile
            .bottles
            .iter()
            .find(|candidate| bottle_key(&candidate.name).ok().as_deref() == Some(&bottle))
        {
            bottle_rule.speed.apply_to(&mut effective);
        }
        rule.speed.apply_to(&mut effective);
        effective.validate()?;
    }
    Ok(())
}

impl SharedSpeedConfig {
    /// Resolve by full bottle + drive-qualified executable identity.
    ///
    /// Case and slash conventions are normalized; neither wildcards nor
    /// basename-only matches are accepted for an executable override.
    pub fn resolve_for(
        &self,
        bottle: &str,
        executable_path: &str,
    ) -> Result<ResolvedSpeed, SpeedProfileError> {
        self.validate()?;
        let bottle = bottle_key(bottle)?;
        let path = game_path_key(executable_path)?;
        let matched_bottle = self
            .bottles
            .iter()
            .find(|rule| bottle_key(&rule.name).ok().as_deref() == Some(&bottle));
        let matched_game = self.games.iter().find(|rule| {
            bottle_key(&rule.bottle).ok().as_deref() == Some(&bottle)
                && game_path_key(&rule.path).ok().as_deref() == Some(&path)
        });

        let mut settings = self.speed.clone();
        if let Some(rule) = matched_bottle {
            rule.speed.apply_to(&mut settings);
        }
        if let Some(rule) = matched_game {
            rule.speed.apply_to(&mut settings);
        }
        settings.validate()?;
        let excluded = matched_bottle.is_some_and(|rule| rule.exclude)
            || matched_game.is_some_and(|rule| rule.exclude);
        if excluded {
            settings.enabled = false;
        }
        Ok(ResolvedSpeed {
            settings,
            excluded,
            matched_bottle: matched_bottle.is_some(),
            matched_game: matched_game.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ResolvedSpeed;
    use crate::speed_config::SharedSpeedConfig;

    const PROFILE: &str = r#"
version = 1
[speed]
enabled = false
initial_multiplier = 1.0

[[bottles]]
name = "Games"
[bottles.speed]
enabled = true
initial_multiplier = 1.5

[[bottles]]
name = "Online"
exclude = true
[bottles.speed]
enabled = true

[[games]]
bottle = "Games"
path = 'C:\Games\One\Game.exe'
[games.speed]
initial_multiplier = 2.0

[[games]]
bottle = "Games"
path = 'C:\Games\Two\Game.exe'
[games.speed]
enabled = false

[[games]]
bottle = "Online"
path = 'C:\Games\Online.exe'
[games.speed]
enabled = true
"#;

    fn resolved(bottle: &str, path: &str) -> ResolvedSpeed {
        SharedSpeedConfig::parse(PROFILE)
            .unwrap()
            .resolve_for(bottle, path)
            .unwrap()
    }

    #[test]
    fn global_bottle_game_precedence_and_path_normalization() {
        let global = resolved("Other", "C:/Games/One/Game.exe");
        assert!(!global.settings.enabled);
        assert!(!global.matched_bottle);
        assert!(!global.matched_game);

        let bottle = resolved(" games ", r"C:\Games\Other.exe");
        assert!(bottle.settings.enabled);
        assert_eq!(bottle.settings.initial_multiplier, 1.5);
        assert!(bottle.matched_bottle);
        assert!(!bottle.matched_game);

        let exact = resolved("GAMES", "c:/games/one/GAME.EXE");
        assert!(exact.settings.enabled);
        assert_eq!(exact.settings.initial_multiplier, 2.0);
        assert!(exact.matched_game);

        let excluded_game = resolved("Games", r"C:\Games\Two\Game.exe");
        assert!(!excluded_game.settings.enabled);
    }

    #[test]
    fn exclusion_overrides_a_more_specific_enable_rule() {
        let blocked = resolved("online", "c:/games/online.exe");
        assert!(blocked.excluded);
        assert!(!blocked.settings.enabled);
        assert!(blocked.matched_game);
    }

    #[test]
    fn duplicate_basenames_in_distinct_paths_and_bottles_are_distinct() {
        let first = resolved("Games", r"C:\Games\One\Game.exe");
        let second = resolved("Games", r"C:\Games\Two\Game.exe");
        let third = resolved("Other", r"C:\Games\One\Game.exe");
        assert_eq!(first.settings.initial_multiplier, 2.0);
        assert!(!second.settings.enabled);
        assert!(!third.settings.enabled);
    }

    #[test]
    fn rejects_case_and_slash_equivalent_duplicate_overrides() {
        let duplicate_bottle = format!("{PROFILE}\n[[bottles]]\nname = 'gAmEs'\n");
        assert!(SharedSpeedConfig::parse(&duplicate_bottle).is_err());
        let duplicate_game =
            format!("{PROFILE}\n[[games]]\nbottle = 'games'\npath = 'c:/GAMES/one/game.exe'\n");
        assert!(SharedSpeedConfig::parse(&duplicate_game).is_err());
    }

    #[test]
    fn rejects_invalid_paths_and_invalid_composed_overrides() {
        for path in [
            r"Game.exe",
            r".\Game.exe",
            r"C:\Games\..\Other.exe",
            r"\\server\share\Game.exe",
            r"C:Game.exe",
        ] {
            assert!(
                SharedSpeedConfig::parse(PROFILE)
                    .unwrap()
                    .resolve_for("Games", path)
                    .is_err()
            );
        }
        let invalid = format!(
            "{PROFILE}\n[[games]]\nbottle = 'Games'\npath = 'c:/games/fresh.exe'\n[games.speed]\ninitial_multiplier = 20.0\n"
        );
        assert!(SharedSpeedConfig::parse(&invalid).is_err());
    }

    #[test]
    fn rejects_unrecognized_override_fields_and_ambiguous_identity() {
        for addition in [
            "\n[[bottles]]\nname = 'Conflict'\nunknown = true\n",
            "\n[[games]]\nbottle = 'Games'\npath = 'Game.exe'\n",
            "\n[[games]]\nbottle = 'Games'\npath = 'C:/Games/./Game.exe'\n",
        ] {
            assert!(SharedSpeedConfig::parse(&format!("{PROFILE}{addition}")).is_err());
        }
    }
}
