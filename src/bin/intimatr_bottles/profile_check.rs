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
    use super::ProfileCheckOptions;
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
