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
    use intimatr::{speed_config::SharedSpeedConfig, speed_profile_load::parse_preview_profile};
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

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
        .map_err(|error| {
            format!("cannot create new launch profile (refusing overwrite): {error}")
        })?;
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

/// Inspect one explicit profile in a bottle's actual C: drive without
/// guessing Z:, inspecting other drives, or modifying a Wine process.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProfilePathOptions {
    bottle: String,
    profile: String,
    bottles_cli: String,
}

#[cfg(target_os = "linux")]
impl ProfilePathOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut bottle = None;
        let mut profile = None;
        let mut bottles_cli = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
                "--profile" if profile.is_none() => profile = Some(value.clone()),
                "--bottles-cli" if bottles_cli.is_none() => bottles_cli = Some(value.clone()),
                _ => {
                    return Err(format!(
                        "unsupported or repeated profile-path option {flag}"
                    ));
                }
            }
        }
        let bottle: String = bottle.ok_or("profile-path requires --bottle NAME")?;
        if bottle.trim().is_empty() {
            return Err("profile-path requires a nonempty bottle name".into());
        }
        let profile = profile.ok_or("profile-path requires --profile ABSOLUTE_LINUX_FILE")?;
        if !std::path::Path::new(&profile).is_absolute() {
            return Err("profile-path requires an absolute Linux profile path".into());
        }
        let bottles_cli = bottles_cli.unwrap_or_else(|| "bottles-cli".to_owned());
        if std::path::Path::new(&bottles_cli)
            .file_name()
            .is_none_or(|name| name != "bottles-cli")
        {
            return Err("expected a native bottles-cli executable".into());
        }
        Ok(Self {
            bottle,
            profile,
            bottles_cli,
        })
    }
}

/// Require literal, unambiguous C: components underneath the verified prefix.
/// Symlinks and parent traversal cannot be used to claim a Windows path.
#[cfg(target_os = "linux")]
fn verified_profile_c_path(
    prefix: &std::path::Path,
    file: &std::path::Path,
) -> Result<String, String> {
    use std::{fs, path::Component};
    if !file.is_absolute() {
        return Err("profile must be an absolute Linux file path".into());
    }
    let drive = prefix.join("drive_c");
    let drive_meta = fs::symlink_metadata(&drive)
        .map_err(|error| format!("cannot inspect verified bottle C: drive: {error}"))?;
    if !drive_meta.is_dir() || drive_meta.file_type().is_symlink() {
        return Err("verified bottle C: drive must be a plain directory".into());
    }
    let drive_real = fs::canonicalize(&drive)
        .map_err(|error| format!("cannot resolve verified bottle C: drive: {error}"))?;
    if !drive_real.starts_with(prefix) {
        return Err("selected bottle C: drive escapes its verified prefix".into());
    }
    let relative = file
        .strip_prefix(&drive)
        .map_err(|_| "profile is not inside the selected bottle's C: drive")?;
    let mut parts = Vec::new();
    let mut cursor = drive;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("unsafe or relative profile path component".into());
        };
        let name = name
            .to_str()
            .ok_or("profile contains a non-UTF-8 Windows path component")?;
        if name.is_empty()
            || name.ends_with('.')
            || name.ends_with(' ')
            || name.chars().any(|c| {
                c.is_control() || matches!(c, '\\' | ':' | '/' | '*' | '?' | '"' | '<' | '>' | '|')
            })
        {
            return Err("profile contains an invalid Windows path component".into());
        }
        cursor.push(name);
        let meta = fs::symlink_metadata(&cursor)
            .map_err(|error| format!("cannot inspect profile path component: {error}"))?;
        if meta.file_type().is_symlink() {
            return Err("profile path contains a symlink; C: translation is ambiguous".into());
        }
        parts.push(name.to_owned());
        if parts.len() > 128 {
            return Err("profile path exceeds bounded Windows component count".into());
        }
    }
    if parts.is_empty() || !cursor.is_file() {
        return Err("profile must name a regular file inside C:".into());
    }
    let resolved = fs::canonicalize(&cursor)
        .map_err(|error| format!("cannot resolve profile file: {error}"))?;
    if !resolved.starts_with(&drive_real) {
        return Err("profile resolves outside the selected C: drive".into());
    }
    Ok(format!("C:\\{}", parts.join("\\")))
}

#[cfg(target_os = "linux")]
pub(super) fn run_profile_path(args: Vec<String>) -> Result<(), String> {
    use intimatr::{
        bottles::parse_bottle_names, bottles_prefix::verify_bottle_prefix,
        speed_profile_load::load_preview_profile,
    };

    let options = ProfilePathOptions::parse(&args)?;
    let list = super::bottles_json_with_command(&options.bottles_cli, &["list", "bottles"])?;
    let bottles = parse_bottle_names(&list)
        .map_err(|error| format!("invalid native Bottles list: {error}"))?;
    if !bottles.contains(&options.bottle) {
        return Err("selected bottle not found in native Bottles inventory".into());
    }
    let root = super::bottles_json_with_command(&options.bottles_cli, &["info", "bottles-path"])?;
    let prefix = intimatr::bottles_prefix::parse_bottles_root(&root)?.join(&options.bottle);
    let identity = verify_bottle_prefix(&root, &options.bottle, &prefix)?;
    let file = std::path::Path::new(&options.profile);
    let windows_path = verified_profile_c_path(&identity.canonical_bottle_prefix, file)?;
    let enabled = load_preview_profile(file)
        .map_err(|error| format!("profile is not acceptable to Windows DLL: {error}"))?
        .is_some();
    let report = serde_json::json!({
        "operation": "read_only_verified_bottle_profile_path",
        "bottle": options.bottle,
        "verified_bottle_prefix": identity.canonical_bottle_prefix,
        "bottle_prefix_association_verified": true,
        "profile_linux_file": options.profile,
        "windows_profile_path": windows_path,
        "profile_validated": true,
        "speed_enabled": enabled,
        "launch_env_name": "INTIMATR_SPEED_PROFILE",
        "running_process_environment_modified": false,
        "runner_verified": false,
        "attachment_attempted": false,
        "gameplay_speed_verified": false,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot encode profile path: {error}"))?
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
    use super::{
        ProfileCheckOptions, ProfileExportOptions, ProfilePathOptions, verified_profile_c_path,
    };
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
        let base = [
            "--config",
            "/tmp/speed.toml",
            "--bottle",
            "Games",
            "--exe",
            r"C:\Games\Game.exe",
        ];
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

    #[test]
    fn path_preflight_requires_specific_bottle_and_absolute_profile() {
        let good = args(&[
            "--bottle",
            "Games",
            "--profile",
            "/tmp/bottle/drive_c/speed.toml",
            "--bottles-cli",
            "/tmp/bottles-cli",
        ]);
        assert_eq!(ProfilePathOptions::parse(&good).unwrap().bottle, "Games");
        for invalid in [
            args(&["--profile", "/tmp/test.toml"]),
            args(&["--bottle", "Games", "--profile", "speed.toml"]),
            [good.clone(), args(&["--profile", "/tmp/other.toml"])].concat(),
            [good.clone(), args(&["--unexpected", "x"])].concat(),
            [good.clone(), args(&["--bottles-cli", "/tmp/fake"])].concat(),
        ] {
            assert!(ProfilePathOptions::parse(&invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn windows_profile_path_refuses_escape_symlink_and_traversal() {
        use std::{
            fs,
            os::unix::fs::symlink,
            sync::atomic::{AtomicUsize, Ordering},
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "intimatr-profile-path-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let prefix = dir.join("Fixture");
        let drive = prefix.join("drive_c");
        fs::create_dir_all(drive.join("Profile Dir")).unwrap();
        let good = drive.join("Profile Dir/speed.toml");
        fs::write(&good, "version = 1\n[speed]\nenabled = true\n").unwrap();
        assert_eq!(
            verified_profile_c_path(&prefix, &good).unwrap(),
            r"C:\Profile Dir\speed.toml"
        );
        let elsewhere = dir.join("outside.toml");
        fs::write(&elsewhere, "version = 1").unwrap();
        assert!(verified_profile_c_path(&prefix, &elsewhere).is_err());
        let link = drive.join("link.toml");
        symlink(&good, &link).unwrap();
        assert!(verified_profile_c_path(&prefix, &link).is_err());
        let escape = drive.join("../drive_c/Profile Dir/speed.toml");
        assert!(verified_profile_c_path(&prefix, &escape).is_err());
        let bad = drive.join("invalid:name.toml");
        fs::write(&bad, "version = 1").unwrap();
        assert!(verified_profile_c_path(&prefix, &bad).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
