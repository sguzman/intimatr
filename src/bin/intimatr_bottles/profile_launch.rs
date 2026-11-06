//! Explicit prelaunch profile handoff to native Bottles' registered-program CLI.
//!
//! This invokes \`bottles-cli run -b NAME -p PROGRAM\` with only the selected
//! process environment changed. It does not edit bottle configuration,
//! install a DLL, or claim that Bottles forwarded the variable to a game.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use intimatr::{
    bottles::{RegisteredProgram, parse_registered_programs},
    speed_config::SharedSpeedConfig,
    speed_policy::game_path_key,
    speed_profile_fingerprint::{sha256_hex, sha256_words},
    speed_profile_load::load_preview_profile,
};

use super::{
    bottle_compat::resolve_registered_file,
    bottles_json_with_command,
    profile_check::verify_bottle_profile_path,
    profile_environment::{
        reject_named_bottle_override, verify_named_bottle_environment,
        verify_named_program_environment, verify_profile_environment,
        verify_registered_program_environment,
    },
};

#[derive(Debug, PartialEq, Eq)]
struct ProfileLaunchOptions {
    config: String,
    bottle: String,
    executable: String,
    program: String,
    profile: String,
    bottles_cli: String,
    experimental_four_timers: bool,
    mangohud: bool,
    mangohud_config: Option<String>,
}

impl ProfileLaunchOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut config = None;
        let mut bottle = None;
        let mut executable = None;
        let mut program = None;
        let mut profile = None;
        let mut bottles_cli = None;
        let mut offline = false;
        let mut launch = false;
        let mut experimental_four_timers = false;
        let mut mangohud = false;
        let mut mangohud_config = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--offline-single-player" if !offline => {
                    offline = true;
                    continue;
                }
                "--launch" if !launch => {
                    launch = true;
                    continue;
                }
                "--experimental-four-timers" if !experimental_four_timers => {
                    experimental_four_timers = true;
                    continue;
                }
                "--mangohud" if !mangohud => {
                    mangohud = true;
                    continue;
                }
                "--offline-single-player"
                | "--launch"
                | "--experimental-four-timers"
                | "--mangohud" => {
                    return Err(format!("duplicate acknowledgment {flag}"));
                }
                _ => {}
            }
            let value = iter
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--config" if config.is_none() => config = Some(value.clone()),
                "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
                "--exe" if executable.is_none() => executable = Some(value.clone()),
                "--program" if program.is_none() => program = Some(value.clone()),
                "--profile" if profile.is_none() => profile = Some(value.clone()),
                "--bottles-cli" if bottles_cli.is_none() => bottles_cli = Some(value.clone()),
                "--mangohud-config" if mangohud_config.is_none() => {
                    mangohud_config = Some(value.clone());
                }
                _ => {
                    return Err(format!(
                        "duplicate or unexpected profile-launch flag: {flag}"
                    ));
                }
            }
        }
        if !offline || !launch {
            return Err(
                "profile-launch requires --offline-single-player and --launch acknowledgments"
                    .into(),
            );
        }
        let config: String = config.ok_or("missing --config")?;
        let profile: String = profile.ok_or("missing --profile")?;
        if !Path::new(&config).is_absolute() || !Path::new(&profile).is_absolute() {
            return Err("source config and launch profile must use absolute Linux paths".into());
        }
        let bottle: String = bottle.ok_or("missing --bottle")?;
        let executable: String = executable.ok_or("missing --exe")?;
        let program: String = program.ok_or("missing --program")?;
        if bottle.trim().is_empty() || program.trim().is_empty() {
            return Err("bottle and registered program names must not be empty".into());
        }
        game_path_key(&executable)
            .map_err(|error| format!("invalid selected game identity: {error}"))?;
        if let Some(path) = &mangohud_config {
            if !mangohud || !Path::new(path).is_absolute() {
                return Err("--mangohud-config requires --mangohud and an absolute path".into());
            }
        }
        let bottles_cli = bottles_cli.unwrap_or_else(|| "bottles-cli".to_owned());
        if Path::new(&bottles_cli)
            .file_name()
            .is_none_or(|name| name != "bottles-cli")
        {
            return Err("expected a native bottles-cli executable".into());
        }
        Ok(Self {
            config,
            bottle,
            executable,
            program,
            profile,
            bottles_cli,
            experimental_four_timers,
            mangohud,
            mangohud_config,
        })
    }
}

/// Require exactly one registered program name whose *full* executable
/// identity agrees with the selected source policy. Never select by basename.
fn verify_registered_program(
    programs: &[RegisteredProgram],
    program_name: &str,
    executable: &str,
) -> Result<(), String> {
    let wanted = game_path_key(executable)
        .map_err(|error| format!("invalid selected executable: {error}"))?;
    let selected = programs
        .iter()
        .filter(|item| item.name == program_name)
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        return Err("selected program name is absent or ambiguous in native Bottles".into());
    }
    let registered = game_path_key(&selected[0].path)
        .map_err(|error| format!("invalid registered executable path: {error}"))?;
    if registered != wanted {
        return Err("registered executable does not match selected full Windows path".into());
    }
    Ok(())
}

/// Accept only a verified Intimatr-owned HUD beside the selected executable.
/// This is an integrity preflight, not a claim of in-game renderer delivery.
fn verified_managed_mangohud_config(
    prefix: &Path,
    windows_executable: &str,
    selected_path: &str,
) -> Result<PathBuf, String> {
    // Non-C: mappings are consulted only after the user explicitly selects
    // the HUD config; no global Z: guessing during ordinary launches.
    let executable = resolve_registered_file(prefix, windows_executable, true)?;
    let directory = executable
        .parent()
        .ok_or("selected executable has no parent directory")?;
    let config = directory.join("MangoHud.conf");
    if Path::new(selected_path) != config {
        return Err("managed MangoHud config must be beside the exact selected executable".into());
    }
    let marker = directory.join("intimatr-mangohud.sha256");
    for path in [&config, &marker] {
        let meta = fs::symlink_metadata(path)
            .map_err(|error| format!("missing managed MangoHud file: {error}"))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("managed MangoHud config or marker is not a regular file".into());
        }
    }
    let bytes = fs::read(&config)
        .map_err(|error| format!("cannot read managed MangoHud config: {error}"))?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err("managed MangoHud config is empty or exceeds 64 KiB".into());
    }
    let expected = fs::read_to_string(&marker)
        .map_err(|error| format!("cannot read MangoHud ownership marker: {error}"))?;
    let expected = expected.trim_end_matches('\n');
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid MangoHud ownership checksum".into());
    }
    if !sha256_hex(&sha256_words(&bytes)).eq_ignore_ascii_case(expected) {
        return Err("MangoHud config checksum changed; refusing launch".into());
    }
    Ok(config)
}

pub(super) fn run_profile_launch(args: Vec<String>) -> Result<(), String> {
    use std::{fs::File, io::Read};

    let options = ProfileLaunchOptions::parse(&args)?;
    let (prefix, windows_profile, enabled) =
        verify_bottle_profile_path(&options.bottle, &options.profile, &options.bottles_cli)?;
    if !enabled {
        return Err("resolved speed profile is explicitly disabled; refusing launch".into());
    }

    // Re-resolve the supplied policy and compare it with the exact TOML file
    // being delivered. A valid profile from a *different* game is not enough.
    let mut source = String::new();
    File::open(&options.config)
        .map_err(|error| format!("cannot open source speed policy: {error}"))?
        .take(1_048_577)
        .read_to_string(&mut source)
        .map_err(|error| format!("cannot read source speed policy: {error}"))?;
    if source.len() > 1_048_576 {
        return Err("source speed policy exceeds 1 MiB".into());
    }
    let policy = SharedSpeedConfig::parse(&source)
        .map_err(|error| format!("invalid source speed policy: {error}"))?;
    let expected = policy
        .resolved_launch_profile(&options.bottle, &options.executable)
        .map_err(|error| format!("cannot resolve selected game speed policy: {error}"))?;
    if !expected.speed.enabled {
        return Err("selected game is excluded or has speed disabled".into());
    }
    let delivered = load_preview_profile(Path::new(&options.profile))
        .map_err(|error| format!("cannot verify delivered profile: {error}"))?
        .ok_or("delivered profile has speed disabled")?;
    if delivered.settings != expected.speed || delivered.hotkeys != expected.hotkeys {
        return Err("launch profile differs from the selected game policy".into());
    }

    let program_json =
        bottles_json_with_command(&options.bottles_cli, &["programs", "-b", &options.bottle])?;
    let registered = parse_registered_programs(&program_json)
        .map_err(|error| format!("invalid selected native Bottles programs: {error}"))?;
    if registered.len() > 1024 {
        return Err("selected bottle has too many registered programs".into());
    }
    verify_registered_program(&registered, &options.program, &options.executable)?;
    verify_registered_program_environment(&program_json, &options.program)?;
    let managed_hud = options
        .mangohud_config
        .as_deref()
        .map(|path| verified_managed_mangohud_config(&prefix, &options.executable, path))
        .transpose()?;

    if options.experimental_four_timers {
        verify_named_program_environment(
            &program_json,
            &options.program,
            "INTIMATR_EXPERIMENTAL_STARTUP_TIMERS",
        )?;
    }
    if options.mangohud {
        for variable in ["MANGOHUD", "INTIMATR_HUD_TELEMETRY"] {
            verify_named_program_environment(&program_json, &options.program, variable)?;
        }
        if managed_hud.is_some() {
            for variable in ["MANGOHUD_CONFIGFILE", "MANGOHUD_CONFIG"] {
                verify_named_program_environment(&program_json, &options.program, variable)?;
            }
        }
    }

    // The selected bottle may restrict inherited host variables. This audit
    // must pass before any game launch and does not attest runtime delivery.
    let inventory_json = bottles_json_with_command(&options.bottles_cli, &["list", "bottles"])?;
    let environment_mode = verify_profile_environment(&inventory_json, &options.bottle)?;
    if options.experimental_four_timers {
        verify_named_bottle_environment(
            &inventory_json,
            &options.bottle,
            "INTIMATR_EXPERIMENTAL_STARTUP_TIMERS",
        )?;
    }
    if options.mangohud {
        for variable in ["MANGOHUD", "INTIMATR_HUD_TELEMETRY"] {
            verify_named_bottle_environment(&inventory_json, &options.bottle, variable)?;
        }
        if managed_hud.is_some() {
            verify_named_bottle_environment(
                &inventory_json,
                &options.bottle,
                "MANGOHUD_CONFIGFILE",
            )?;
            reject_named_bottle_override(&inventory_json, &options.bottle, "MANGOHUD_CONFIG")?;
        }
    }

    // Bottles' documented run command has no per-launch environment option.
    // Supply a process-local variable without modifying the bottle's
    // persistent settings. The CLI exit status is NOT gameplay verification.
    let mut command = Command::new(&options.bottles_cli);
    command
        .arg("run")
        .arg("-b")
        .arg(&options.bottle)
        .arg("-p")
        .arg(&options.program)
        .env("INTIMATR_SPEED_PROFILE", &windows_profile)
        .env_remove("INTIMATR_EXPERIMENTAL_STARTUP_TIMERS");
    if options.experimental_four_timers {
        command.env("INTIMATR_EXPERIMENTAL_STARTUP_TIMERS", "1");
    }
    // Vulkan MangoHud activation is opt-in and scoped to this exact launch.
    // The telemetry publisher itself does not arm any timing hooks.
    if options.mangohud {
        command
            .env("MANGOHUD", "1")
            .env("INTIMATR_HUD_TELEMETRY", "1");
    }
    if let Some(config) = &managed_hud {
        command
            .env_remove("MANGOHUD_CONFIG")
            .env("MANGOHUD_CONFIGFILE", config);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot invoke native Bottles run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "native Bottles run failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }

    let report = serde_json::json!({
        "operation": "explicit_registered_bottles_profile_launch",
        "selected_bottle": options.bottle,
        "registered_program": options.program,
        "registered_windows_executable": options.executable,
        "verified_bottle_prefix": prefix,
        "windows_launch_profile": windows_profile,
        "source_game_policy_verified": true,
        "profile_file_matching_source_verified": true,
        "bottles_environment_policy_preflight_passed": true,
        "registered_program_environment_preflight_passed": true,
        "bottles_reported_profile_environment_mode": environment_mode,
        "bottles_environment_delivery_verified_in_game": false,
        "bottles_run_cli_exited_successfully": true,
        "game_process_started_verified": false,
        "profile_reached_game_verified": false,
        "experimental_startup_timer_activation_requested": options.experimental_four_timers,
        "experimental_startup_timer_activation_verified_in_game": false,
        "mangohud_vulkan_activation_requested": options.mangohud,
        "mangohud_telemetry_export_requested": options.mangohud,
        "mangohud_managed_config_verified_before_launch": managed_hud.is_some(),
        "mangohud_managed_config_host_path": managed_hud,
        "mangohud_rendering_verified_in_game": false,
        "dll_auto_loaded": false,
        "gameplay_speed_verified": false,
        "bottle_configuration_modified": false,
        "attachment_attempted": false,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot encode profile launch result: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProfileLaunchOptions, verify_registered_program};
    use intimatr::bottles::RegisteredProgram;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn launch_requires_both_consent_flags_and_exact_selection() {
        let base = args(&[
            "--config",
            "/tmp/global.toml",
            "--bottle",
            "Fixture",
            "--exe",
            r"C:\Games\Game.exe",
            "--program",
            "Offline Game",
            "--profile",
            "/tmp/bottle/drive_c/speed.toml",
            "--bottles-cli",
            "/tmp/bottles-cli",
        ]);
        let mut allowed = base.clone();
        allowed.extend(args(&["--offline-single-player", "--launch"]));
        assert_eq!(
            ProfileLaunchOptions::parse(&allowed).unwrap().program,
            "Offline Game"
        );
        let opted_in = [allowed.clone(), args(&["--experimental-four-timers"])].concat();
        let hud = [allowed.clone(), args(&["--mangohud"])].concat();
        let configured = [
            hud.clone(),
            args(&["--mangohud-config", "/tmp/MangoHud.conf"]),
        ]
        .concat();
        assert_eq!(
            ProfileLaunchOptions::parse(&configured)
                .unwrap()
                .mangohud_config
                .as_deref(),
            Some("/tmp/MangoHud.conf")
        );
        assert!(ProfileLaunchOptions::parse(&hud).unwrap().mangohud);
        assert!(!ProfileLaunchOptions::parse(&allowed).unwrap().mangohud);
        assert!(
            ProfileLaunchOptions::parse(&opted_in)
                .unwrap()
                .experimental_four_timers
        );
        assert!(
            !ProfileLaunchOptions::parse(&allowed)
                .unwrap()
                .experimental_four_timers
        );
        for invalid in [
            base.clone(),
            [base.clone(), args(&["--launch"])].concat(),
            [base.clone(), args(&["--offline-single-player"])].concat(),
            [allowed.clone(), args(&["--launch"])].concat(),
            [opted_in.clone(), args(&["--experimental-four-timers"])].concat(),
            [hud.clone(), args(&["--mangohud"])].concat(),
            [
                allowed.clone(),
                args(&["--mangohud-config", "/tmp/MangoHud.conf"]),
            ]
            .concat(),
            [hud.clone(), args(&["--mangohud-config", "MangoHud.conf"])].concat(),
            [
                configured.clone(),
                args(&["--mangohud-config", "/tmp/other.conf"]),
            ]
            .concat(),
            [allowed.clone(), args(&["--profile", "/tmp/other.toml"])].concat(),
            [allowed.clone(), args(&["--unknown", "value"])].concat(),
        ] {
            assert!(ProfileLaunchOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn registered_name_and_exact_windows_path_must_both_match() {
        let program = RegisteredProgram {
            name: "Offline Game".into(),
            executable: "Game.exe".into(),
            path: r"C:\Games\Game.exe".into(),
            arguments: String::new(),
        };
        assert!(
            verify_registered_program(&[program.clone()], "Offline Game", "c:/games/GAME.exe")
                .is_ok()
        );
        assert!(
            verify_registered_program(&[program.clone()], "Offline Game", "C:/Other/Game.exe")
                .is_err()
        );
        assert!(verify_registered_program(&[program.clone()], "Another", &program.path).is_err());
        assert!(
            verify_registered_program(
                &[program.clone(), program.clone()],
                "Offline Game",
                &program.path
            )
            .is_err()
        );
    }
}
