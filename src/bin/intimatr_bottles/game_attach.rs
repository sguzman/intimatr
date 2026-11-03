//! Explicit single-player game attachment from the native Bottles manager.
//! This is a user-initiated mutation of one running Windows process. It must
//! never be selected automatically from a plan or all-games filter.

use std::{fs, path::Path, process::Command};

use super::live_match::inspect_live_selection;

#[derive(Debug, PartialEq, Eq)]
struct GameAttachOptions {
    live_args: Vec<String>,
    attach_helper: String,
    expected_creation_time_100ns: String,
}

impl GameAttachOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut live_args = Vec::new();
        let mut attach_helper = None;
        let mut expected_creation_time_100ns = None;
        let mut consent = false;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--offline-single-player" if !consent => consent = true,
                "--offline-single-player" => {
                    return Err("duplicate offline-single-player acknowledgment".into());
                }
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
                        return Err("selected process creation time must be a canonical positive FILETIME".into());
                    }
                    expected_creation_time_100ns = Some(value.clone());
                }
                "--created-100ns" => return Err("duplicate --created-100ns".into()),
                _ => live_args.push(arg.clone()),
            }
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
        })
    }
}

pub(super) fn run_game_attach(args: Vec<String>) -> Result<(), String> {
    let options = GameAttachOptions::parse(&args)?;
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

    // The Windows helper opens a HANDLE to the selected PID and checks the
    // image and live architecture again under its per-target mutex before
    // writing a DLL path; the snapshot alone is never attachment authority.
    let output = Command::new(&selected.runner)
        .arg(&attach_helper)
        .arg("--pid")
        .arg(candidate.selected_windows_pid.to_string())
        .arg("--exe")
        .arg(&candidate.selected_executable_path)
        .arg("--created-100ns")
        .arg(creation)
        .arg("--offline-single-player")
        .env("WINEPREFIX", &selected.prefix)
        .env("WINEDEBUG", "-all")
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
    let result = serde_json::json!({
        "operation": "explicit_offline_single_player_game_attachment",
        "selected_bottle": candidate.selected_bottle,
        "selected_windows_pid": candidate.selected_windows_pid,
        "selected_executable_path": candidate.selected_executable_path,
        "verified_process_creation_time_100ns": creation,
        "verified_wine_prefix": selected.prefix,
        "verified_wine_runner": selected.runner,
        "intimatr_module_loaded": true,
        "speed_controls_verified": false,
        "attachment_attempted": true,
        "offline_single_player_acknowledged": true
    });
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
            "--attach-helper", "/tmp/wine-game-attach-helper.exe",
            "--bottle", "Games", "--pid", "42",
            "--exe", r"C:\Games\Game.exe",
        ];
        for invalid in ["0", "-1", "1.5", "0001", "18446744073709551616"] {
            let mut values = args(&base);
            values.extend(args(&["--created-100ns", invalid, "--offline-single-player"]));
            assert!(GameAttachOptions::parse(&values).is_err());
        }
        let mut missing = args(&base);
        missing.push("--offline-single-player".into());
        assert!(GameAttachOptions::parse(&missing).is_err());

        let mut duplicated = args(&base);
        duplicated.extend(args(&[
            "--created-100ns", "133700000000000001",
            "--created-100ns", "133700000000000001",
            "--offline-single-player",
        ]));
        assert!(GameAttachOptions::parse(&duplicated).is_err());
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
