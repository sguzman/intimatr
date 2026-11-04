//! Read-only per-bottle and executable speed profile inspection.

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProfileCheckOptions {
    config: String,
    bottle: String,
    executable: String,
}

#[cfg(target_os = "linux")]
impl ProfileCheckOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = None;
        let mut bottle = None;
        let mut executable = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|v| !v.is_empty() && !v.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--config" if config.is_none() => config = Some(value.clone()),
                "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
                "--exe" if executable.is_none() => executable = Some(value.clone()),
                _ => return Err(format!("duplicate or unsupported profile flag: {flag}")),
            }
        }
        Ok(Self {
            config: config.ok_or("missing --config")?,
            bottle: bottle.ok_or("missing --bottle")?,
            executable: executable.ok_or("missing --exe")?,
        })
    }
}

/// A caller may explicitly prepare one launch-time profile file. Its
/// destination must be chosen by the caller; no prefix or DOS drive is
/// guessed, and a preexisting file is never overwritten.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProfileExportOptions {
    selection: ProfileCheckOptions,
    output: String,
}

#[cfg(target_os = "linux")]
impl ProfileExportOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut selection_args = Vec::new();
        let mut output = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            if flag == "--output" {
                if output.replace(value.clone()).is_some() {
                    return Err("duplicate --output path".into());
                }
            } else {
                selection_args.push(flag.clone());
                selection_args.push(value.clone());
            }
        }
        let selection = ProfileCheckOptions::parse(&selection_args)?;
        let output = output.ok_or("profile-export requires --output ABSOLUTE_LINUX_FILE")?;
        if !std::path::Path::new(&output).is_absolute() {
            return Err("profile-export output must be an absolute Linux file path".into());
        }
        Ok(Self { selection, output })
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run_profile_export(args: Vec<String>) -> Result<(), String> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    use intimatr::{
        speed_config::SharedSpeedConfig,
        speed_profile_load::parse_preview_profile,
    };

    let options = ProfileExportOptions::parse(&args)?;
    let source = std::fs::read_to_string(&options.selection.config)
        .map_err(|error| format!("cannot read speed profile: {error}"))?;
    if source.len() > 1_048_576 {
        return Err("source speed profile exceeds 1 MiB".into());
    }
    let profile = SharedSpeedConfig::parse(&source)
        .map_err(|error| format!("invalid speed profile: {error}"))?;
    let launch = profile
        .resolved_launch_profile(&options.selection.bottle, &options.selection.executable)
        .map_err(|error| format!("cannot resolve game launch profile: {error}"))?;
    let rendered = toml::to_string_pretty(&launch)
        .map_err(|error| format!("cannot render launch profile: {error}"))?;
    if rendered.len() > 1_048_576 {
        return Err("resolved launch profile exceeds 1 MiB".into());
    }
    let parsed = parse_preview_profile(&rendered)
        .map_err(|error| format!("resolved profile is not accepted by Windows DLL: {error}"))?;
    if parsed.is_some() != launch.speed.enabled {
        return Err("resolved launch enable state disagrees with DLL profile loader".into());
    }

    // create_new is atomic: even existing symlinks are refused. Mode 0600
    // avoids exposing a selected user's profile to other local accounts.
    // No game files, Bottles registrations or Wine processes are touched.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&options.output)
        .map_err(|error| format!("cannot create new launch profile (refusing overwrite): {error}"))?;
    file.write_all(rendered.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot finish launch profile output: {error}"))?;
    let report = serde_json::json!({
        "operation": "explicit_launch_profile_export",
        "bottle": options.selection.bottle,
        "executable": options.selection.executable,
        "output_linux_file": options.output,
        "resolved_speed_enabled": launch.speed.enabled,
        "override_rules_in_output": launch.bottles.len() + launch.games.len(),
        "requires_absolute_windows_visible_launch_path": true,
        "launch_env_name": "INTIMATR_SPEED_PROFILE",
        "running_process_environment_modified": false,
        "attachment_attempted": false,
        "gameplay_speed_verified": false,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot encode export report: {error}"))?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn run_profile_check(args: Vec<String>) -> Result<(), String> {
    use intimatr::speed_config::SharedSpeedConfig;
    let options = ProfileCheckOptions::parse(&args)?;
    let contents = std::fs::read_to_string(&options.config)
        .map_err(|error| format!("cannot read speed profile: {error}"))?;
    let profile = SharedSpeedConfig::parse(&contents)
        .map_err(|error| format!("invalid speed profile: {error}"))?;
    let result = profile
        .resolve_for(&options.bottle, &options.executable)
        .map_err(|error| format!("cannot resolve target speed: {error}"))?;
    let output = serde_json::json!({
        "operation": "read_only_profile_resolution",
        "bottle": options.bottle,
        "executable": options.executable,
        "resolved": result,
        "hotkeys": &profile.hotkeys,
        "hotkey_delivery": "not_wired_to_game_input",
        "dll_loaded": false,
        "attachment_attempted": false,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("cannot encode profile result: {error}"))?
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod profile_check_tests {
    use super::{ProfileCheckOptions, ProfileExportOptions};
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn requires_all_fields_and_rejects_duplicated_flags() {
        for invalid in [
            args(&[]),
            args(&["--config", "/tmp/profile.toml", "--bottle", "Games"]),
            args(&[
                "--config",
                "/tmp/profile.toml",
                "--bottle",
                "Games",
                "--exe",
                "C:/Games/Game.exe",
                "--exe",
                "C:/Other.exe",
            ]),
            args(&[
                "--config",
                "/tmp/profile.toml",
                "--bottle",
                "Games",
                "--exe",
                "C:/Games/Game.exe",
                "--unknown",
                "yes",
            ]),
        ] {
            assert!(ProfileCheckOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn export_requires_explicit_absolute_destination_and_rejects_duplicates() {
        let base = ["--config", "/tmp/speed.toml", "--bottle", "Games", "--exe", r"C:\Games\Game.exe"];
        let mut valid = args(&base);
        valid.extend(args(&["--output", "/tmp/launch-profile.toml"]));
        let parsed = ProfileExportOptions::parse(&valid).unwrap();
        assert_eq!(parsed.output, "/tmp/launch-profile.toml");
        for invalid in [
            args(&base),
            [args(&base), args(&["--output", "relative.toml"])].concat(),
            [valid.clone(), args(&["--output", "/tmp/other.toml"])].concat(),
            [valid.clone(), args(&["--unknown", "yes"])].concat(),
        ] {
            assert!(ProfileExportOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn parses_read_only_profile_inspection_without_bottles() {
        let options = ProfileCheckOptions::parse(&args(&[
            "--config",
            "/tmp/speed global.toml",
            "--bottle",
            "Games",
            "--exe",
            r"C:\Games\Game.exe",
        ]))
        .unwrap();
        assert_eq!(options.bottle, "Games");
        assert_eq!(options.executable, r"C:\Games\Game.exe");
    }
}
