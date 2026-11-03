//! Disposable Wine fixture attachment CLI, not a general-game injector.
use std::process::Command;

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct ProbeAttachOptions {
    pid: u32,
    prefix: String,
    helper: String,
    runner: String,
}

#[cfg(target_os = "linux")]
impl ProbeAttachOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut pid = None;
        let mut prefix = None;
        let mut helper = None;
        let mut runner = None;
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            let value = iter
                .next()
                .filter(|item| !item.is_empty() && !item.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--pid" if pid.is_none() => {
                    let parsed: u32 = value.parse().map_err(|_| "invalid target PID")?;
                    if parsed == 0 {
                        return Err("target PID must be positive".into());
                    }
                    pid = Some(parsed);
                }
                "--prefix" if prefix.is_none() => prefix = Some(value.clone()),
                "--helper" if helper.is_none() => helper = Some(value.clone()),
                "--runner" if runner.is_none() => runner = Some(value.clone()),
                _ => return Err(format!("duplicate or unsupported option {flag}")),
            }
        }
        let runner =
            runner.ok_or("missing --runner: select explicit Wine binary for this prefix")?;
        intimatr::bottles_runner::validate_explicit_wine_runner_arg(&runner)?;
        Ok(Self {
            pid: pid.ok_or("missing --pid")?,
            prefix: prefix.ok_or("missing --prefix")?,
            helper: helper.ok_or("missing --helper")?,
            runner,
        })
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run_probe_attach(args: Vec<String>) -> Result<(), String> {
    use std::{fs, path::Path};

    let opts = ProbeAttachOptions::parse(&args)?;
    let prefix =
        fs::canonicalize(&opts.prefix).map_err(|error| format!("invalid Wine prefix: {error}"))?;
    // Wine can defer writing registry snapshots while a prefix is active.
    // Its initialized drive tree is a more reliable non-mutating check.
    if !prefix.is_dir() || !prefix.join("drive_c").is_dir() {
        return Err("Wine prefix must already contain an initialized drive_c".into());
    }
    let helper = fs::canonicalize(&opts.helper)
        .map_err(|error| format!("invalid Windows helper path: {error}"))?;
    if !helper.is_file()
        || !helper
            .file_name()
            .is_some_and(|name| name == "wine-attach-helper.exe")
    {
        return Err("refusing helper other than wine-attach-helper.exe".into());
    }
    let dll = helper
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("intimatr.dll");
    if !dll.is_file() {
        return Err("test helper directory does not contain intimatr.dll".into());
    }
    // The helper itself checks the Windows target image and refuses every PID
    // whose executable is not our disposable wine-attach-target.exe fixture.
    let verified_runner = intimatr::bottles_runner::resolve_explicit_wine_binary(&opts.runner)?;
    let result = Command::new(&verified_runner)
        .arg(&helper)
        .arg(opts.pid.to_string())
        .env("WINEPREFIX", &prefix)
        .env("WINEDEBUG", "-all")
        .output()
        .map_err(|error| format!("cannot start specified Wine runner: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "test-only Wine helper failed (exit {:?}): {}",
            result.status.code(),
            String::from_utf8_lossy(&result.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }
    println!(
        "{}",
        serde_json::json!({
            "operation": "fixture_only_remote_load",
            "target_pid": opts.pid,
            "wine_prefix": prefix,
            "helper": helper,
            "helper_exited_successfully": true,
            "game_attachment": false
        })
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod probe_tests {
    use super::ProbeAttachOptions;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn requires_explicit_target_prefix_and_known_helper() {
        for invalid in [
            args(&[]),
            args(&[
                "--pid",
                "32",
                "--prefix",
                "/tmp/owned prefix",
                "--helper",
                "/tmp/wine-attach-helper.exe",
            ]),
            args(&["--pid", "0", "--prefix", "/tmp", "--helper", "test.exe"]),
            args(&[
                "--pid", "garbage", "--prefix", "/tmp", "--helper", "test.exe",
            ]),
            args(&["--pid", "3", "--prefix", "/tmp"]),
            args(&[
                "--pid", "3", "--prefix", "/tmp", "--helper", "test.exe", "--runner", "wine",
            ]),
            args(&[
                "--pid", "3", "--prefix", "/tmp", "--helper", "test.exe", "--pid", "4",
            ]),
            args(&[
                "--pid",
                "3",
                "--prefix",
                "/tmp",
                "--helper",
                "test.exe",
                "--unknown",
                "1",
            ]),
        ] {
            assert!(ProbeAttachOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn parses_test_only_command_without_shell_interpolation() {
        let parsed = ProbeAttachOptions::parse(&args(&[
            "--pid",
            "32",
            "--prefix",
            "/tmp/owned prefix",
            "--helper",
            "/tmp/wine-attach-helper.exe",
            "--runner",
            "/opt/patched-runner/bin/wine",
        ]))
        .expect("valid test request");
        assert_eq!(parsed.pid, 32);
        assert_eq!(parsed.prefix, "/tmp/owned prefix");
        assert_eq!(parsed.runner, "/opt/patched-runner/bin/wine");
    }
}
