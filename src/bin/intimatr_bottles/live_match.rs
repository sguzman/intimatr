//! Read-only native Bottles and scoped Wine live-process reconciliation.
use super::{WindowsProcessesOptions, bottles_json_with_command, query_windows_process_snapshot};

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct LiveMatchOptions {
    bottle: String,
    prefix: Option<String>,
    helper: String,
    runner: String,
    bottles_cli: String,
    selected_pid: Option<u32>,
    selected_executable: Option<String>,
}

#[cfg(target_os = "linux")]
impl LiveMatchOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut bottle = None;
        let mut prefix = None;
        let mut helper = None;
        let mut runner = None;
        let mut bottles_cli = None;
        let mut selected_pid = None;
        let mut selected_executable = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
                "--prefix" if prefix.is_none() => prefix = Some(value.clone()),
                "--helper" if helper.is_none() => helper = Some(value.clone()),
                "--runner" if runner.is_none() => runner = Some(value.clone()),
                "--bottles-cli" if bottles_cli.is_none() => bottles_cli = Some(value.clone()),
                "--pid" if selected_pid.is_none() => {
                    let pid: u32 = value.parse().map_err(|_| "invalid Windows PID")?;
                    if pid == 0 {
                        return Err("Windows PID must be positive".into());
                    }
                    selected_pid = Some(pid);
                }
                "--exe" if selected_executable.is_none() => {
                    selected_executable = Some(value.clone());
                }
                _ => {
                    return Err(format!(
                        "duplicate or unsupported live-match option: {flag}"
                    ));
                }
            }
        }
        let bottle: String = bottle.ok_or("missing --bottle")?;
        if bottle.trim().is_empty() {
            return Err("bottle name must not be empty".into());
        }
        if selected_pid.is_some() != selected_executable.is_some() {
            return Err(
                "--pid and --exe are required together for explicit candidate review".into(),
            );
        }
        if let (Some(pid), Some(path)) = (selected_pid, selected_executable.as_deref()) {
            intimatr::bottles_preflight::validate_explicit_candidate_selection(pid, path)?;
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
            prefix,
            helper: helper.ok_or("missing --helper")?,
            runner: runner
                .ok_or("missing --runner: select the explicit Wine binary for this bottle")?,
            bottles_cli,
            selected_pid,
            selected_executable,
        })
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run_live_match(args: Vec<String>) -> Result<(), String> {
    use intimatr::{
        bottles::{BottleInventory, parse_bottle_names, parse_registered_programs},
        bottles_attachment_readiness::review_attachment_readiness,
        bottles_live_matches::match_registered_programs,
        bottles_prefix::verify_bottle_prefix,
        bottles_preflight::assess_explicit_candidate,
        bottles_runner::{audit_recorded_runner, audit_runner_metadata},
    };

    let options = LiveMatchOptions::parse(&args)?;
    // Require both a registered bottle name and a verified direct prefix
    // child of Bottles' configured root. This still grants no attach access.
    let bottle_list_json = bottles_json_with_command(&options.bottles_cli, &["list", "bottles"])?;
    let names = parse_bottle_names(&bottle_list_json)
        .map_err(|error| format!("invalid native Bottles list: {error}"))?;
    if !names.iter().any(|name| name == &options.bottle) {
        return Err("selected bottle not found in native Bottles inventory".into());
    }
    // Upstream Bottles reports this as a *plain path* even with --json.
    let configured_bottles_root =
        bottles_json_with_command(&options.bottles_cli, &["info", "bottles-path"])?;
    // Default to the direct bottle directory from Bottles' own path.
    // A supplied --prefix is still checked against that exact location.
    let prefix_source = if options.prefix.is_some() {
        "explicit_verified"
    } else {
        "configured_bottles_root"
    };
    let selected_prefix = options
        .prefix
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(configured_bottles_root.trim_end_matches(['\r', '\n']))
                .join(&options.bottle)
        });
    let ownership =
        verify_bottle_prefix(&configured_bottles_root, &options.bottle, &selected_prefix)?;
    let mut runner_audit = audit_runner_metadata(&bottle_list_json, &options.bottle)?;
    audit_recorded_runner(&mut runner_audit, std::path::Path::new(&options.runner))?;
    let programs = parse_registered_programs(&bottles_json_with_command(
        &options.bottles_cli,
        &["programs", "-b", &options.bottle],
    )?)
    .map_err(|error| format!("invalid native Bottles programs: {error}"))?;
    let (prefix, snapshot) = query_windows_process_snapshot(&WindowsProcessesOptions {
        prefix: selected_prefix.to_string_lossy().into_owned(),
        helper: options.helper,
        runner: options.runner,
    })?;
    let preview = match_registered_programs(
        &BottleInventory {
            bottle: options.bottle.clone(),
            programs,
        },
        &snapshot,
    );
    let selected_candidate = options
        .selected_pid
        .zip(options.selected_executable.as_deref())
        .map(|(pid, exe)| assess_explicit_candidate(&preview, pid, exe))
        .transpose()?;
    let attachment_readiness = selected_candidate
        .as_ref()
        .map(|candidate| review_attachment_readiness(candidate, &ownership, &runner_audit));
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "operation": "read_only_live_bottles_wine_match",
            "selected_bottle": options.bottle,
            "resolved_prefix": prefix,
            "prefix_source": prefix_source,
            "bottle_prefix_association_verified": ownership.bottle_prefix_association_verified,
            "configured_bottles_root": ownership.canonical_bottles_root,
            "runner_verified": runner_audit.runner_path_verified,
            "runner_audit": runner_audit,
            "game_process_identity_verified": ownership.game_process_identity_verified,
            "windows_process_count": snapshot.processes.len(),
            "matches": preview.matches,
            "selected_candidate": selected_candidate,
            "attachment_readiness": attachment_readiness,
            "safe_to_attach": false,
            "attachment_attempted": false
        }))
        .map_err(|error| format!("cannot serialize read-only live-match: {error}"))?
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod live_match_tests {
    use super::LiveMatchOptions;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn requires_bottle_and_probe_but_not_manual_prefix() {
        for invalid in [
            args(&[]),
            args(&["--bottle", "Games", "--helper", "wine-process-list.exe"]),
            args(&["--bottle", "Games", "--prefix", "/tmp"]),
            args(&["--helper", "wine-process-list.exe"]),
            args(&[
                "--bottle",
                "Games",
                "--prefix",
                "/tmp",
                "--helper",
                "helper.exe",
                "--bottles-cli",
                "/tmp/fake-program",
            ]),
            args(&[
                "--bottle",
                "Games",
                "--prefix",
                "/tmp",
                "--helper",
                "helper.exe",
                "--bottle",
                "Another",
            ]),
            args(&[
                "--bottle",
                "Games",
                "--helper",
                "helper.exe",
                "--runner",
                "/usr/bin/wine",
                "--pid",
                "42",
            ]),
            args(&[
                "--bottle",
                "Games",
                "--helper",
                "helper.exe",
                "--runner",
                "/usr/bin/wine",
                "--exe",
                r"C:\Games\Game.exe",
            ]),
            args(&[
                "--bottle",
                "Games",
                "--helper",
                "helper.exe",
                "--runner",
                "/usr/bin/wine",
                "--pid",
                "0",
                "--exe",
                r"C:\Games\Game.exe",
            ]),
        ] {
            assert!(LiveMatchOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn may_derive_prefix_from_bottles_root_without_user_path() {
        let parsed = LiveMatchOptions::parse(&args(&[
            "--bottle",
            "My Games",
            "--helper",
            "/tmp/wine-process-list.exe",
            "--runner",
            "/opt/custom-patched-wine/bin/wine",
        ]))
        .unwrap();
        assert_eq!(parsed.prefix, None);
        assert_eq!(parsed.bottle, "My Games");
    }

    #[test]
    fn accepts_explicit_mock_cli_and_wine_runner_without_shell_interpolation() {
        let parsed = LiveMatchOptions::parse(&args(&[
            "--bottle",
            "My Games",
            "--prefix",
            "/tmp/native prefix",
            "--helper",
            "/tmp/wine-process-list.exe",
            "--bottles-cli",
            "/tmp/bottles-cli",
            "--runner",
            "/home/user/patch runner/wine",
        ]))
        .unwrap();
        assert_eq!(parsed.bottle, "My Games");
        assert_eq!(parsed.prefix.as_deref(), Some("/tmp/native prefix"));
        assert_eq!(parsed.bottles_cli, "/tmp/bottles-cli");
        assert_eq!(parsed.runner, "/home/user/patch runner/wine");
    }

    #[test]
    fn explicit_preflight_requires_pid_and_full_windows_executable() {
        let parsed = LiveMatchOptions::parse(&args(&[
            "--bottle",
            "Games",
            "--helper",
            "/tmp/wine-process-list.exe",
            "--runner",
            "/usr/bin/wine",
            "--pid",
            "42",
            "--exe",
            r"C:\Games\Game.exe",
        ]))
        .unwrap();
        assert_eq!(parsed.selected_pid, Some(42));
        assert_eq!(
            parsed.selected_executable.as_deref(),
            Some(r"C:\Games\Game.exe")
        );
        assert!(
            LiveMatchOptions::parse(&args(&[
                "--bottle",
                "Games",
                "--helper",
                "/tmp/wine-process-list.exe",
                "--runner",
                "/usr/bin/wine",
                "--pid",
                "42",
                "--exe",
                r"C:Relative\Game.exe",
            ]))
            .is_err()
        );
        assert!(
            LiveMatchOptions::parse(&args(&[
                "--bottle",
                "Games",
                "--helper",
                "/tmp/wine-process-list.exe",
                "--runner",
                "/usr/bin/wine",
                "--pid",
                "42",
                "--pid",
                "43",
                "--exe",
                r"C:\Games\Game.exe",
            ]))
            .is_err()
        );
    }
}
