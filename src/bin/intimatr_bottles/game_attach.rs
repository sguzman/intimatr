//! Explicit single-player game attachment from the native Bottles manager.
//! This is a user-initiated mutation of one running Windows process. It must
//! never be selected automatically from a plan or all-games filter.

use std::{fs, path::Path, process::Command};

use super::live_match::inspect_live_selection;
use intimatr::pe_timer_imports::TimerImportCoverage;

#[derive(Debug, PartialEq, Eq)]
struct GameAttachOptions {
    live_args: Vec<String>,
    attach_helper: String,
    expected_creation_time_100ns: String,
    experimental_four_timers: bool,
    rate_preset: Option<String>,
    rate_milli: Option<u32>,
    step_action: Option<String>,
    comparison_profile: Option<String>,
}

impl GameAttachOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut live_args = Vec::new();
        let mut attach_helper = None;
        let mut expected_creation_time_100ns = None;
        let mut consent = false;
        let mut experimental_four_timers = false;
        let mut rate_preset = None;
        let mut rate_milli = None;
        let mut step_action = None;
        let mut comparison_profile = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--offline-single-player" if !consent => consent = true,
                "--offline-single-player" => {
                    return Err("duplicate offline-single-player acknowledgment".into());
                }
                "--preset" if rate_preset.is_none() => {
                    let value = iter.next().ok_or("missing --preset value")?;
                    if !matches!(value.as_str(), "half" | "normal" | "double") {
                        return Err("speed preset must be half, normal or double".into());
                    }
                    rate_preset = Some(value.clone());
                }
                "--preset" => return Err("duplicate --preset".into()),
                "--multiplier" if rate_milli.is_none() => {
                    let value = iter.next().ok_or("missing --multiplier value")?;
                    rate_milli = Some(parse_multiplier_milli(value)?);
                }
                "--multiplier" => return Err("duplicate --multiplier".into()),
                "--action" if step_action.is_none() => {
                    let value = iter.next().ok_or("missing --action value")?;
                    if !matches!(value.as_str(), "increase" | "decrease" | "toggle" | "reset") {
                        return Err(
                            "speed action must be increase, decrease, toggle or reset".into()
                        );
                    }
                    step_action = Some(value.clone());
                }
                "--action" => return Err("duplicate --action".into()),
                "--experimental-four-timers" if !experimental_four_timers => {
                    experimental_four_timers = true;
                }
                "--experimental-four-timers" => {
                    return Err("duplicate experimental-four-timers acknowledgment".into());
                }
                "--profile" if comparison_profile.is_none() => {
                    let path = iter.next().ok_or("missing --profile path")?;
                    if !Path::new(path).is_absolute() {
                        return Err("comparison profile must be an absolute Linux file path".into());
                    }
                    comparison_profile = Some(path.clone());
                }
                "--profile" => return Err("duplicate --profile".into()),
                "--attach-helper" if attach_helper.is_none() => {
                    let path = iter.next().ok_or("missing --attach-helper path")?;
                    if !Path::new(path).is_absolute() {
                        return Err(
                            "game attach helper requires an absolute executable path".into()
                        );
                    }
                    attach_helper = Some(path.clone());
                }
                "--attach-helper" => return Err("duplicate --attach-helper".into()),
                "--created-100ns" if expected_creation_time_100ns.is_none() => {
                    let value = iter.next().ok_or("missing --created-100ns value")?;
                    let parsed = value
                        .parse::<u64>()
                        .map_err(|_| "invalid selected process creation time")?;
                    if parsed == 0 || parsed.to_string() != *value {
                        return Err(
                            "selected process creation time must be a canonical positive FILETIME"
                                .into(),
                        );
                    }
                    expected_creation_time_100ns = Some(value.clone());
                }
                "--created-100ns" => return Err("duplicate --created-100ns".into()),
                _ => live_args.push(arg.clone()),
            }
        }
        if rate_preset.is_some() && rate_milli.is_some() {
            return Err("choose exactly one --preset or --multiplier".into());
        }
        if step_action.is_some() && (rate_preset.is_some() || rate_milli.is_some()) {
            return Err("choose only --action for step-game-speed".into());
        }
        if !consent {
            return Err("explicit --offline-single-player acknowledgment is required".into());
        }
        let attach_helper = attach_helper.ok_or("missing --attach-helper")?;
        let expected_creation_time_100ns = expected_creation_time_100ns
            .ok_or("attach-game requires --created-100ns from the selected live-match report")?;
        if !live_args.iter().any(|arg| arg == "--pid")
            || !live_args.iter().any(|arg| arg == "--exe")
        {
            return Err("attach-game requires both --pid and --exe for one explicit game".into());
        }
        // Full candidate identity, one bottle, one PID, and one registered
        // executable are required later by the common live matcher.
        Ok(Self {
            live_args,
            attach_helper,
            expected_creation_time_100ns,
            experimental_four_timers,
            rate_preset,
            rate_milli,
            step_action,
            comparison_profile,
        })
    }
}

/// Convert a user-entered multiplier into the bounded integer remote ABI.
/// Parse the decimal lexeme itself: parsing through f64 first can round away
/// tiny excess precision and mistakenly accept four or more decimal places.
/// No exponents, signs, locale punctuation, or trailing decimal separator.
fn parse_multiplier_milli(raw: &str) -> Result<u32, String> {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || whole.len() > 2
        || (whole.starts_with('0') && whole.len() > 1)
        || !whole.bytes().all(|digit| digit.is_ascii_digit())
        || !fraction.bytes().all(|digit| digit.is_ascii_digit())
        || (raw.contains('.') && fraction.is_empty())
    {
        return Err("speed multiplier must be an unsigned decimal number".into());
    }
    if fraction.len() > 3 {
        return Err("speed multiplier must have at most three decimal places".into());
    }
    let whole = whole
        .parse::<u32>()
        .map_err(|_| "invalid speed multiplier")?;
    let fraction_milli = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u32>()
            .map_err(|_| "invalid speed multiplier")?
            * 10_u32.pow(3 - fraction.len() as u32)
    };
    let milli = whole
        .checked_mul(1000)
        .and_then(|part| part.checked_add(fraction_milli))
        .ok_or("speed multiplier is outside bounded thousandth range")?;
    if !(100..=10_000).contains(&milli) {
        return Err("speed multiplier must be finite and between 0.1 and 10".into());
    }
    Ok(milli)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameAction {
    Attach,
    Stop,
    Status,
    ArmTimers,
    SetRate,
    Step,
}

pub(super) fn run_game_attach(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::Attach)
}

pub(super) fn run_game_stop(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::Stop)
}

pub(super) fn run_game_status(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::Status)
}

pub(super) fn run_game_timer_activation(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::ArmTimers)
}

pub(super) fn run_game_rate(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::SetRate)
}

pub(super) fn run_game_step(args: Vec<String>) -> Result<(), String> {
    run_game_action(args, GameAction::Step)
}

/// A known-incomplete on-disk PE import set is enough to reject an arm
/// before starting the mutation-capable Windows helper. Unknown static
/// coverage is never converted to a fabricated zero; the DLL will still
/// independently check its resolved in-memory imports before installation.
fn validate_static_timer_imports(
    action: GameAction,
    coverage: Option<TimerImportCoverage>,
) -> Result<(), String> {
    if action != GameAction::ArmTimers {
        return Ok(());
    }
    if let Some(coverage) = coverage
        && !coverage.is_complete()
    {
        return Err(format!(
            "refusing timer activation: selected main executable static PE is missing imports: {}; no remote helper launched",
            coverage.missing_names().join(", ")
        ));
    }
    Ok(())
}

fn run_game_action(args: Vec<String>, action: GameAction) -> Result<(), String> {
    let options = GameAttachOptions::parse(&args)?;
    let needs_experimental = matches!(
        action,
        GameAction::ArmTimers | GameAction::SetRate | GameAction::Step
    );
    if action == GameAction::ArmTimers && !options.experimental_four_timers {
        return Err("arm-game-timers requires explicit --experimental-four-timers opt-in".into());
    }
    if action == GameAction::SetRate && !options.experimental_four_timers {
        return Err("set-game-speed requires explicit --experimental-four-timers opt-in".into());
    }
    if action == GameAction::Step && !options.experimental_four_timers {
        return Err("step-game-speed requires explicit --experimental-four-timers opt-in".into());
    }
    if !needs_experimental && options.experimental_four_timers {
        return Err(
            "--experimental-four-timers is only valid for arm-game-timers, set-game-speed or step-game-speed".into(),
        );
    }
    if action == GameAction::SetRate
        && options.rate_preset.is_none()
        && options.rate_milli.is_none()
    {
        return Err(
            "set-game-speed requires --preset half|normal|double or --multiplier VALUE".into(),
        );
    }
    if action != GameAction::SetRate
        && (options.rate_preset.is_some() || options.rate_milli.is_some())
    {
        return Err("--preset and --multiplier are only valid for set-game-speed".into());
    }
    if action == GameAction::Step && options.step_action.is_none() {
        return Err("step-game-speed requires --action increase|decrease|toggle|reset".into());
    }
    if action != GameAction::Step && options.step_action.is_some() {
        return Err("--action is only valid for step-game-speed".into());
    }
    if action != GameAction::Status && options.comparison_profile.is_some() {
        return Err("--profile is only valid for read-only status-game".into());
    }
    let attach_helper = fs::canonicalize(&options.attach_helper)
        .map_err(|error| format!("cannot find game attach helper: {error}"))?;
    if !attach_helper.is_file()
        || attach_helper
            .file_name()
            .is_none_or(|name| name != "wine-game-attach-helper.exe")
    {
        return Err("expected the dedicated wine-game-attach-helper.exe".into());
    }
    let dll = attach_helper
        .parent()
        .ok_or("game helper directory missing")?
        .join("intimatr.dll");
    let dll_meta = fs::metadata(&dll)
        .map_err(|error| format!("Intimatr DLL is missing beside game attach helper: {error}"))?;
    if !dll_meta.is_file() || dll_meta.len() == 0 {
        return Err("expected nonempty Intimatr DLL beside game attach helper".into());
    }

    // Obtains fresh process evidence from the selected Wine prefix, validates
    // native Bottles runner identity and exact registered full image path.
    let selected = inspect_live_selection(options.live_args)?;
    let candidate = selected
        .candidate
        .ok_or("attach-game requires --pid and --exe for one selected game")?;
    if !selected.prefix_verified {
        return Err("cannot prove selected bottle owns the supplied Wine prefix".into());
    }
    if !selected.runner_verified {
        return Err("cannot verify native Bottles configured Wine runner for this game".into());
    }
    let creation = candidate
        .process_creation_time_100ns
        .as_deref()
        .ok_or("cannot verify selected game process creation time in Wine snapshot")?;
    if creation != options.expected_creation_time_100ns {
        return Err("selected process creation time differs from the fresh Wine snapshot".into());
    }
    if !candidate.registered_identity_verified_at_snapshot
        || !candidate.pid_observed_in_selected_wineserver_snapshot
        || !candidate.image_observed_in_selected_wineserver_snapshot
        || candidate.x64_dll_compatible != Some(true)
        || candidate.status != "registered_x64_candidate_for_review_only"
    {
        return Err(format!(
            "refusing game attach: candidate PID/image/architecture was not verified ({})",
            candidate.status
        ));
    }

    validate_static_timer_imports(action, candidate.timer_import_coverage)?;

    // Optional status-only comparison against one caller-specified file.
    // Verify its C: path under the same freshly selected bottle prefix.
    // The digest comes from the bytes this read validated, not a later
    // filesystem read. Never infer a Windows path from Z:.
    let expected_profile = if let Some(profile_path) = options.comparison_profile.as_deref() {
        let path = Path::new(profile_path);
        let windows_path = super::profile_check::verified_profile_c_path(&selected.prefix, path)?;
        let profile = intimatr::speed_profile_load::load_preview_profile(path)
            .map_err(|error| format!("cannot read comparison speed profile: {error}"))?
            .ok_or("comparison profile must contain enabled speed settings")?;
        Some((
            intimatr::speed_profile_fingerprint::sha256_hex(&profile.sha256),
            windows_path,
        ))
    } else {
        None
    };

    // The Windows helper opens a HANDLE to the selected PID and checks the
    // image and live architecture again under its per-target mutex before
    // writing a DLL path; the snapshot alone is never attachment authority.
    let mut command = Command::new(&selected.runner);
    command
        .arg(&attach_helper)
        .arg("--pid")
        .arg(candidate.selected_windows_pid.to_string())
        .arg("--exe")
        .arg(&candidate.selected_executable_path)
        .arg("--created-100ns")
        .arg(creation)
        .arg("--offline-single-player")
        .env("WINEPREFIX", &selected.prefix)
        .env("WINEDEBUG", "-all");
    match action {
        GameAction::Attach => {}
        GameAction::Stop => {
            command.arg("--shutdown-only");
        }
        GameAction::Status => {
            command.arg("--status-only");
        }
        GameAction::ArmTimers => {
            command
                .arg("--arm-four-timers-only")
                .arg("--experimental-four-timers");
        }
        GameAction::SetRate => {
            if let Some(preset) = options.rate_preset.as_deref() {
                command.arg("--set-game-preset-only").arg(preset);
            } else {
                command.arg("--set-game-rate-milli-only").arg(
                    options
                        .rate_milli
                        .ok_or("missing requested speed multiplier")?
                        .to_string(),
                );
            }
            command.arg("--experimental-four-timers");
        }
        GameAction::Step => {
            command
                .arg("--step-game-speed-only")
                .arg(
                    options
                        .step_action
                        .as_deref()
                        .ok_or("missing speed action")?,
                )
                .arg("--experimental-four-timers");
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot launch selected Wine game helper: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "game helper rejected attach (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(600)
                .collect::<String>()
        ));
    }
    // For status, the target-side lifecycle export is queried through the
    // same exact-identity helper; an absent module is a distinct valid state.
    // Never equate module presence with a Running speed engine.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let runtime_state = if action == GameAction::Status {
        let mut states = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_RUNTIME_STATE="));
        let state = states
            .next()
            .ok_or("game status helper did not return an authoritative runtime state")?;
        if states.next().is_some()
            || !matches!(
                state,
                "absent"
                    | "detached"
                    | "attached"
                    | "bootstrapping"
                    | "running"
                    | "stopping"
                    | "stopped"
                    | "failed"
            )
        {
            return Err("game status helper returned invalid or duplicate runtime states".into());
        }
        Some(state)
    } else {
        let expected_marker = match action {
            GameAction::Stop => "INTIMATR_REMOTE_RUNTIME_STOPPED=1",
            GameAction::ArmTimers => "INTIMATR_REMOTE_FOUR_TIMERS_INSTALLED=1",
            GameAction::SetRate | GameAction::Step => "INTIMATR_REMOTE_GAME_RATE_MILLI=",
            GameAction::Attach => "INTIMATR_REMOTE_RUNTIME_RUNNING=1",
            GameAction::Status => return Err("status response unexpectedly missing".into()),
        };
        if !stdout.lines().any(|line| {
            if matches!(action, GameAction::SetRate | GameAction::Step) {
                line.starts_with(expected_marker)
            } else {
                line == expected_marker
            }
        }) {
            return Err(
                "game helper returned without independent target runtime state proof".into(),
            );
        }
        None
    };
    let result = if action == GameAction::Status {
        let state = runtime_state.ok_or("missing selected target status")?;
        let mut hooks = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_TIMER_HOOK_STATE="));
        let hook_state = hooks
            .next()
            .ok_or("game status helper did not return a target timer-hook state")?;
        if hooks.next().is_some()
            || !matches!(
                hook_state,
                "none" | "installed_four_timers" | "cleanup_pending"
            )
            || (state == "absent" && hook_state != "none")
            || (state != "running" && hook_state == "installed_four_timers")
        {
            return Err("inconsistent or duplicate target timer-hook state".into());
        }
        let mut rates = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_GAME_RATE_MILLI="));
        let rate_milli = rates
            .next()
            .ok_or("game status helper did not return authoritative hook clock rate")?
            .parse::<u32>()
            .map_err(|_| "invalid game hook clock rate")?;
        if rates.next().is_some()
            || (hook_state == "installed_four_timers") != (rate_milli != 0)
            || (rate_milli != 0 && !(100..=10_000).contains(&rate_milli))
        {
            return Err("target hook rate conflicts with timer installation status".into());
        }
        let mut profiles = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_SPEED_PROFILE_STATE="));
        let profile_state = profiles
            .next()
            .ok_or("game status helper did not return target speed-profile state")?;
        if profiles.next().is_some()
            || !matches!(
                profile_state,
                "absent" | "accepted" | "rejected_or_disabled" | "unavailable"
            )
            || (state == "absent" && profile_state != "unavailable")
            || (state == "running" && profile_state == "unavailable")
        {
            return Err("invalid or contradictory target speed-profile state".into());
        }
        let mut hashes = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_SPEED_PROFILE_SHA256="));
        let sha256_text = hashes
            .next()
            .ok_or("game status helper did not report target profile digest")?;
        if hashes.next().is_some()
            || (profile_state == "accepted"
                && (sha256_text.len() != 64
                    || !sha256_text
                        .bytes()
                        .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit))))
            || (profile_state != "accepted" && sha256_text != "unavailable")
        {
            return Err("invalid or contradictory target profile SHA-256 evidence".into());
        }
        let target_profile_sha256 = (profile_state == "accepted").then_some(sha256_text);
        let profile_contents_match_verified = target_profile_sha256.is_some()
            && expected_profile
                .as_ref()
                .is_some_and(|(expected, _)| Some(expected.as_str()) == target_profile_sha256);
        let mut inventory = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_TIMER_IMPORT_MASK="));
        let inventory_label = inventory
            .next()
            .ok_or("game status helper did not return timer import inventory")?;
        if inventory.next().is_some() {
            return Err("duplicate target main-image timer import inventory".into());
        }
        let import_mask = if inventory_label == "unknown" {
            None
        } else {
            let parsed = inventory_label
                .parse::<u8>()
                .map_err(|_| "invalid target timer import coverage")?;
            if parsed > 15 {
                return Err("invalid target timer import coverage bits".into());
            }
            Some(parsed)
        };
        if (state == "absent" && import_mask.is_some())
            || (hook_state == "installed_four_timers" && import_mask != Some(15))
        {
            return Err("target timer import inventory contradicts hook/module status".into());
        }
        let present = import_mask.map(|mask| mask == 15);
        let known_imports = import_mask.map(|mask| {
            [
                "QueryPerformanceCounter",
                "GetTickCount64",
                "GetTickCount",
                "timeGetTime",
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(index, name)| (mask & (1_u8 << index) != 0).then_some(name))
            .collect::<Vec<_>>()
        });
        let missing_imports = import_mask.map(|mask| {
            [
                "QueryPerformanceCounter",
                "GetTickCount64",
                "GetTickCount",
                "timeGetTime",
            ]
            .into_iter()
            .enumerate()
            .filter_map(|(index, name)| (mask & (1_u8 << index) == 0).then_some(name))
            .collect::<Vec<_>>()
        });
        let virtual_rate = (rate_milli != 0).then_some(f64::from(rate_milli) / 1000.0);
        serde_json::json!({
            "operation": "explicit_offline_single_player_game_status",
            "selected_bottle": candidate.selected_bottle,
            "selected_windows_pid": candidate.selected_windows_pid,
            "selected_executable_path": candidate.selected_executable_path,
            "verified_process_creation_time_100ns": creation,
            "verified_wine_prefix": selected.prefix,
            "verified_wine_runner": selected.runner,
            "intimatr_module_loaded": state != "absent",
            "runtime_state": state,
            "runtime_running_verified": state == "running",
            "runtime_stopped_verified": state == "stopped",
            "target_speed_profile_state": profile_state,
            "explicit_speed_profile_accepted_in_target": state == "running" && profile_state == "accepted",
            "target_speed_profile_sha256": target_profile_sha256,
            "comparison_profile_windows_path": expected_profile.as_ref().map(|(_, path)| path),
            "profile_contents_match_verified": profile_contents_match_verified,
            "profile_path_identity_verified": false,
            "timer_hook_state": hook_state,
            "timer_hooks_installed_verified": state == "running" && hook_state == "installed_four_timers",
            "installed_virtual_clock_multiplier": virtual_rate,
            "main_image_four_timer_imports_present": present,
            "static_pe_timer_import_coverage": candidate.timer_import_coverage,
            "main_image_timer_imports_observed": known_imports,
            "main_image_timer_imports_missing": missing_imports,
            "gameplay_speed_verified": false,
            "speed_controls_verified": false,
            "offline_single_player_acknowledged": true
        })
    } else if action == GameAction::SetRate {
        let expected = if let Some(milli) = options.rate_milli {
            milli
        } else {
            match options.rate_preset.as_deref() {
                Some("half") => 500_u32,
                Some("normal") => 1000,
                Some("double") => 2000,
                _ => return Err("unsupported selected-game speed preset".into()),
            }
        };
        let marker = format!("INTIMATR_REMOTE_GAME_RATE_MILLI={expected}");
        if !stdout.lines().any(|line| line == marker) {
            return Err("game helper did not independently confirm the expected clock rate".into());
        }
        serde_json::json!({
            "operation": "explicit_offline_single_player_game_speed_preset",
            "selected_bottle": candidate.selected_bottle,
            "selected_windows_pid": candidate.selected_windows_pid,
            "selected_executable_path": candidate.selected_executable_path,
            "verified_process_creation_time_100ns": creation,
            "verified_wine_prefix": selected.prefix,
            "verified_wine_runner": selected.runner,
            "intimatr_module_loaded": true,
            "runtime_running_verified": true,
            "timer_hooks_installed_verified": true,
            "installed_virtual_clock_multiplier": f64::from(expected) / 1000.0,
            "gameplay_speed_verified": false,
            "speed_controls_verified": false,
            "experimental_four_timers_acknowledged": true,
            "offline_single_player_acknowledged": true
        })
    } else if action == GameAction::Step {
        let mut rates = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("INTIMATR_REMOTE_GAME_RATE_MILLI="));
        let observed = rates
            .next()
            .ok_or("game helper did not confirm changed virtual clock rate")?
            .parse::<u32>()
            .map_err(|_| "game helper returned invalid changed clock rate")?;
        if rates.next().is_some() || !(100..=10_000).contains(&observed) {
            return Err("game helper returned duplicate or out-of-range changed clock rate".into());
        }
        serde_json::json!({
            "operation": "explicit_offline_single_player_game_speed_action",
            "selected_bottle": candidate.selected_bottle,
            "selected_windows_pid": candidate.selected_windows_pid,
            "selected_executable_path": candidate.selected_executable_path,
            "verified_process_creation_time_100ns": creation,
            "verified_wine_prefix": selected.prefix,
            "verified_wine_runner": selected.runner,
            "speed_action": options.step_action,
            "intimatr_module_loaded": true,
            "runtime_running_verified": true,
            "timer_hooks_installed_verified": true,
            "installed_virtual_clock_multiplier": f64::from(observed) / 1000.0,
            "gameplay_speed_verified": false,
            "speed_controls_verified": false,
            "experimental_four_timers_acknowledged": true,
            "offline_single_player_acknowledged": true
        })
    } else if action == GameAction::ArmTimers {
        serde_json::json!({
            "operation": "explicit_offline_single_player_game_timer_activation",
            "selected_bottle": candidate.selected_bottle,
            "selected_windows_pid": candidate.selected_windows_pid,
            "selected_executable_path": candidate.selected_executable_path,
            "verified_process_creation_time_100ns": creation,
            "verified_wine_prefix": selected.prefix,
            "verified_wine_runner": selected.runner,
            "intimatr_module_loaded": true,
            "runtime_running_verified": true,
            "timer_hooks_installed_verified": true,
            "timer_hook_state": "installed_four_timers",
            "installed_virtual_clock_multiplier": 1.0,
            "gameplay_speed_verified": false,
            "speed_controls_verified": false,
            "activation_attempted": true,
            "experimental_four_timers_acknowledged": true,
            "offline_single_player_acknowledged": true
        })
    } else if action == GameAction::Stop {
        serde_json::json!({
            "operation": "explicit_offline_single_player_game_shutdown",
            "selected_bottle": candidate.selected_bottle,
            "selected_windows_pid": candidate.selected_windows_pid,
            "selected_executable_path": candidate.selected_executable_path,
            "verified_process_creation_time_100ns": creation,
            "verified_wine_prefix": selected.prefix,
            "verified_wine_runner": selected.runner,
            "intimatr_module_loaded": true,
            "intimatr_module_unloaded": false,
            "runtime_stopped_verified": true,
            "speed_controls_verified": false,
            "shutdown_attempted": true,
            "offline_single_player_acknowledged": true
        })
    } else {
        serde_json::json!({
        "operation": "explicit_offline_single_player_game_attachment",
        "selected_bottle": candidate.selected_bottle,
        "selected_windows_pid": candidate.selected_windows_pid,
        "selected_executable_path": candidate.selected_executable_path,
        "verified_process_creation_time_100ns": creation,
        "verified_wine_prefix": selected.prefix,
        "verified_wine_runner": selected.runner,
        "intimatr_module_loaded": true,
        "runtime_running_verified": true,
        "speed_controls_verified": false,
        "attachment_attempted": true,
        "offline_single_player_acknowledged": true
        })
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&result)
            .map_err(|error| format!("cannot serialize game attachment report: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn comparison_profile_is_explicit_and_status_only() {
        let base = args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--created-100ns",
            "133700000000000001",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--offline-single-player",
        ]);
        let with_file = [base.clone(), args(&["--profile", "/tmp/speed.toml"])].concat();
        assert_eq!(
            GameAttachOptions::parse(&with_file)
                .unwrap()
                .comparison_profile
                .as_deref(),
            Some("/tmp/speed.toml")
        );
        assert!(
            run_game_action(with_file, GameAction::Attach)
                .unwrap_err()
                .contains("only valid for read-only status-game")
        );
        for invalid in [
            [base.clone(), args(&["--profile", "speed.toml"])].concat(),
            [base.clone(), args(&["--profile"])].concat(),
            [base, args(&["--profile", "/tmp/a", "--profile", "/tmp/b"])].concat(),
        ] {
            assert!(GameAttachOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn no_implicit_or_unselected_game_attach() {
        for input in [
            args(&[]),
            args(&["--offline-single-player"]),
            args(&["--attach-helper", "/tmp/wine-game-attach-helper.exe"]),
            args(&[
                "--attach-helper",
                "/tmp/wine-game-attach-helper.exe",
                "--offline-single-player",
            ]),
            args(&[
                "--attach-helper",
                "wine-game-attach-helper.exe",
                "--offline-single-player",
            ]),
            args(&[
                "--attach-helper",
                "/tmp/wine-game-attach-helper.exe",
                "--offline-single-player",
                "--offline-single-player",
            ]),
        ] {
            assert!(GameAttachOptions::parse(&input).is_err());
        }
    }

    #[test]
    fn refuses_missing_duplicate_and_malformed_process_generation_selectors() {
        let base = [
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--bottle",
            "Games",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
        ];
        for invalid in ["0", "-1", "1.5", "0001", "18446744073709551616"] {
            let mut values = args(&base);
            values.extend(args(&[
                "--created-100ns",
                invalid,
                "--offline-single-player",
            ]));
            assert!(GameAttachOptions::parse(&values).is_err());
        }
        let mut missing = args(&base);
        missing.push("--offline-single-player".into());
        assert!(GameAttachOptions::parse(&missing).is_err());

        let mut duplicated = args(&base);
        duplicated.extend(args(&[
            "--created-100ns",
            "133700000000000001",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
        ]));
        assert!(GameAttachOptions::parse(&duplicated).is_err());
    }

    #[test]
    fn incomplete_static_pe_imports_reject_arming_before_remote_helper_launch() {
        let mut coverage = TimerImportCoverage::default();
        assert!(validate_static_timer_imports(GameAction::Attach, Some(coverage)).is_ok());
        assert!(validate_static_timer_imports(GameAction::Status, Some(coverage)).is_ok());
        assert!(validate_static_timer_imports(GameAction::SetRate, Some(coverage)).is_ok());
        assert!(validate_static_timer_imports(GameAction::ArmTimers, None).is_ok());
        let rejected =
            validate_static_timer_imports(GameAction::ArmTimers, Some(coverage)).unwrap_err();
        assert!(rejected.contains("QueryPerformanceCounter"));
        assert!(rejected.contains("timeGetTime"));
        assert!(rejected.contains("no remote helper launched"));

        coverage.query_performance_counter = true;
        coverage.get_tick_count64 = true;
        coverage.get_tick_count = true;
        coverage.time_get_time = true;
        coverage.all_four_static_names = true;
        assert!(validate_static_timer_imports(GameAction::ArmTimers, Some(coverage)).is_ok());
    }

    #[test]
    fn experimental_timer_activation_requires_independent_consent() {
        let base = args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--bottle",
            "Games",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
        ]);
        assert!(
            run_game_timer_activation(base.clone())
                .unwrap_err()
                .contains("requires explicit --experimental-four-timers")
        );
        let mut armed = base;
        armed.push("--experimental-four-timers".into());
        assert!(
            GameAttachOptions::parse(&armed)
                .unwrap()
                .experimental_four_timers
        );
        for action in [GameAction::Attach, GameAction::Stop, GameAction::Status] {
            assert!(
                run_game_action(armed.clone(), action)
                    .unwrap_err()
                    .contains("only valid for arm-game-timers")
            );
        }
        armed.push("--experimental-four-timers".into());
        assert!(
            GameAttachOptions::parse(&armed)
                .unwrap_err()
                .contains("duplicate experimental-four-timers")
        );
    }

    #[test]
    fn preset_commands_require_separate_opt_in_and_bounded_values() {
        let mut args = args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--bottle",
            "Games",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
            "--preset",
            "half",
        ]);
        assert!(
            run_game_rate(args.clone())
                .unwrap_err()
                .contains("requires explicit --experimental-four-timers")
        );
        args.push("--experimental-four-timers".into());
        let parsed = GameAttachOptions::parse(&args).unwrap();
        assert_eq!(parsed.rate_preset.as_deref(), Some("half"));
        assert!(parsed.experimental_four_timers);
        assert!(
            run_game_timer_activation(args.clone())
                .unwrap_err()
                .contains("--preset and --multiplier are only valid")
        );
        args.extend(["--preset".into(), "normal".into()]);
        assert!(GameAttachOptions::parse(&args).is_err());
        args.truncate(args.len() - 2);
        let mut invalid = args.clone();
        let preset = invalid.iter_mut().find(|value| *value == "half").unwrap();
        *preset = "unbounded".into();
        assert!(
            GameAttachOptions::parse(&invalid)
                .unwrap_err()
                .contains("half, normal or double")
        );
    }

    #[test]
    fn numeric_speed_multiplier_requires_precise_bounded_configured_step() {
        for (raw, expected) in [
            ("0.1", 100),
            ("0.100", 100),
            ("0.101", 101),
            ("0.25", 250),
            ("0.75", 750),
            ("1", 1000),
            ("1.001", 1001),
            ("1.5", 1500),
            ("2.0", 2000),
            ("9.999", 9999),
            ("10", 10_000),
            ("10.000", 10_000),
        ] {
            assert_eq!(parse_multiplier_milli(raw).unwrap(), expected);
        }
        for invalid in [
            "",
            "0",
            "-1",
            "0.000",
            "0.099",
            "10.001",
            "10.0001",
            "nan",
            "NaN",
            "inf",
            "1.0005",
            "1.0000",
            "1.0000000000000000000001",
            "1e0",
            "1e309",
            "faster",
            "1,5",
            "1.",
            ".5",
            "+1",
            "01.0",
            "1..5",
            " 1",
            "1 ",
            "100",
            "18446744073709551616",
        ] {
            assert!(parse_multiplier_milli(invalid).is_err(), "{invalid}");
        }
        let mut values = args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
            "--experimental-four-timers",
            "--multiplier",
            "1.5",
        ]);
        let parsed = GameAttachOptions::parse(&values).unwrap();
        assert_eq!(parsed.rate_milli, Some(1500));
        assert_eq!(parsed.rate_preset, None);
        values.extend(args(&["--preset", "double"]));
        assert!(
            GameAttachOptions::parse(&values)
                .unwrap_err()
                .contains("exactly one")
        );
        values.truncate(values.len() - 2);
        values.extend(args(&["--multiplier", "2"]));
        assert!(GameAttachOptions::parse(&values).is_err());
    }

    #[test]
    fn selected_game_steps_require_one_action_and_independent_opt_in() {
        let mut values = args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--bottle",
            "Games",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
            "--action",
            "increase",
        ]);
        assert!(
            run_game_step(values.clone())
                .unwrap_err()
                .contains("requires explicit --experimental-four-timers")
        );
        values.push("--experimental-four-timers".into());
        assert_eq!(
            GameAttachOptions::parse(&values)
                .unwrap()
                .step_action
                .as_deref(),
            Some("increase")
        );
        for action in ["increase", "decrease", "toggle", "reset"] {
            let mut selected = values.clone();
            *selected
                .iter_mut()
                .find(|value| *value == "increase")
                .unwrap() = action.into();
            assert_eq!(
                GameAttachOptions::parse(&selected)
                    .unwrap()
                    .step_action
                    .as_deref(),
                Some(action)
            );
        }
        for other in [GameAction::Attach, GameAction::Stop, GameAction::SetRate] {
            assert!(run_game_action(values.clone(), other).is_err());
        }
        let mut missing = values.clone();
        missing.truncate(missing.len() - 3);
        missing.push("--experimental-four-timers".into());
        assert!(
            run_game_step(missing)
                .unwrap_err()
                .contains("requires --action")
        );
        values.extend(args(&["--action", "decrease"]));
        assert!(GameAttachOptions::parse(&values).is_err());
        values.truncate(values.len() - 2);
        let mut incompatible = values;
        incompatible.extend(args(&["--multiplier", "2.0"]));
        assert!(GameAttachOptions::parse(&incompatible).is_err());
    }

    #[test]
    fn retains_exact_selected_candidate_flags_without_implicit_selection() {
        let parsed = GameAttachOptions::parse(&args(&[
            "--attach-helper",
            "/tmp/wine-game-attach-helper.exe",
            "--bottle",
            "Games",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
            "--created-100ns",
            "133700000000000001",
            "--offline-single-player",
        ]))
        .unwrap();
        assert_eq!(parsed.attach_helper, "/tmp/wine-game-attach-helper.exe");
        assert_eq!(parsed.expected_creation_time_100ns, "133700000000000001");
        assert_eq!(
            parsed.live_args,
            args(&[
                "--bottle",
                "Games",
                "--pid",
                "42",
                "--exe",
                r"C:\Games\Game.exe"
            ])
        );
    }
}
