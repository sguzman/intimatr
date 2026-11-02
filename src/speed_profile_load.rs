//! Explicit shared-profile handoff for the DLL's preview command layer.
//!
//! The native Bottles manager must supply a Windows-visible path through
//! INTIMATR_SPEED_PROFILE. Never guess Linux homedir drive mapping, bottle
//! identity or an executable basename. Malformed/unavailable paths fail closed
//! for speed only and cannot prevent legacy scanner/debugger startup.

use std::{fs::File, io::Read, path::Path, sync::Arc};

use thiserror::Error;
use tracing::{info, warn};

use crate::{
    command::CommandExecutor,
    speed_commands::SpeedPreviewExecutor,
    speed_config::{SharedSpeedConfig, SpeedProfileError, SpeedSettings},
    speed_hotkeys::SpeedHotkeyConfig,
};

pub const SPEED_PROFILE_ENV: &str = "INTIMATR_SPEED_PROFILE";
const MAX_PROFILE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone)]
pub struct PreviewProfile {
    pub settings: SpeedSettings,
    pub hotkeys: SpeedHotkeyConfig,
}

#[derive(Debug, Error)]
pub enum PreviewProfileLoadError {
    #[error("cannot open or read shared speed profile: {0}")]
    Io(#[from] std::io::Error),
    #[error("shared speed profile exceeds 1 MiB")]
    TooLarge,
    #[error(transparent)]
    Invalid(#[from] SpeedProfileError),
    #[error(
        "bottle/game override resolution requires verified bottle and full executable identity"
    )]
    UnresolvedOverrides,
}

/// Use only globally resolved speed settings. Any bottle or game rule is
/// refused until the launch/attach layer provides its verified identity.
pub fn parse_preview_profile(
    content: &str,
) -> Result<Option<PreviewProfile>, PreviewProfileLoadError> {
    if content.len() as u64 > MAX_PROFILE_BYTES {
        return Err(PreviewProfileLoadError::TooLarge);
    }
    let global = SharedSpeedConfig::parse(content)?;
    if !global.bottles.is_empty() || !global.games.is_empty() {
        return Err(PreviewProfileLoadError::UnresolvedOverrides);
    }
    if !global.speed.enabled {
        return Ok(None);
    }
    Ok(Some(PreviewProfile {
        settings: global.speed,
        hotkeys: global.hotkeys,
    }))
}

pub fn load_preview_profile(
    path: &Path,
) -> Result<Option<PreviewProfile>, PreviewProfileLoadError> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(PreviewProfileLoadError::TooLarge);
    }
    let text = String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    parse_preview_profile(&text)
}

/// Wrap the established command executor without mutating any game or bottle.
/// Opt-in is the environment variable *and* enabled=true in validated TOML.
/// Missing/bad profiles leave speed unconfigured; all other commands proceed.
pub fn wrap_explicit_preview_from_env(inner: Arc<dyn CommandExecutor>) -> Arc<SpeedPreviewExecutor> {
    let Some(path) = std::env::var_os(SPEED_PROFILE_ENV) else {
        return Arc::new(SpeedPreviewExecutor::disabled(inner));
    };
    match load_preview_profile(Path::new(&path)) {
        Ok(Some(profile)) => match SpeedPreviewExecutor::with_profile(
            Arc::clone(&inner),
            &profile.settings,
            &profile.hotkeys,
            "explicit-shared-profile",
        ) {
            Ok(executor) => {
                info!(
                    "explicit shared speed preview configured; game timing hooks remain disabled"
                );
                Arc::new(executor)
            }
            Err(error) => {
                warn!(error = %error, "shared speed preview disabled after initialization error");
                Arc::new(SpeedPreviewExecutor::disabled(inner))
            }
        },
        Ok(None) => {
            info!("shared speed profile has enabled=false; speed commands remain disabled");
            Arc::new(SpeedPreviewExecutor::disabled(inner))
        }
        Err(error) => {
            warn!(error = %error, "shared speed preview disabled after profile rejection");
            Arc::new(SpeedPreviewExecutor::disabled(inner))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_and_implicit_settings_fail_closed() {
        assert!(parse_preview_profile("version = 1").unwrap().is_none());
        assert!(
            parse_preview_profile("version = 1\n[speed]\nenabled = false")
                .unwrap()
                .is_none()
        );
        assert!(parse_preview_profile("version = 2").is_err());
    }

    #[test]
    fn enabled_explicit_global_profile_is_preview_only() {
        let profile = parse_preview_profile(
            "version = 1\n[speed]\nenabled = true\ninitial_multiplier = 1.5\n[hotkeys]\nincrease = 'Ctrl+F7'",
        )
        .unwrap()
        .unwrap();
        assert!(profile.settings.enabled);
        assert_eq!(profile.settings.initial_multiplier, 1.5);
        assert_eq!(profile.hotkeys.increase.as_deref(), Some("Ctrl+F7"));
    }

    #[test]
    fn refuses_unresolved_bottle_or_game_overrides() {
        let bottle = "version = 1\n[speed]\nenabled = true\n[[bottles]]\nname = 'Games'";
        assert!(matches!(
            parse_preview_profile(bottle),
            Err(PreviewProfileLoadError::UnresolvedOverrides)
        ));
        let game = "version = 1\n[[games]]\nbottle = 'Games'\npath = 'C:\\Games\\Game.exe'";
        assert!(matches!(
            parse_preview_profile(game),
            Err(PreviewProfileLoadError::UnresolvedOverrides)
        ));
    }

    #[test]
    fn overlarge_and_invalid_hotkey_profiles_are_refused() {
        let huge = " ".repeat((MAX_PROFILE_BYTES + 1) as usize);
        assert!(matches!(
            parse_preview_profile(&huge),
            Err(PreviewProfileLoadError::TooLarge)
        ));
        let collision = "version = 1\n[speed]\nenabled = true\n[hotkeys]\nreset = 'F10'";
        assert!(parse_preview_profile(collision).is_err());
    }
}
