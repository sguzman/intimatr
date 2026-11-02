//! Native Linux controller for Bottles inventory and carefully bounded tests.
//!
//! scan/plan are read-only. probe-attach explicitly starts an owned Windows
//! helper that refuses every process except wine-attach-target.exe.
//! No ordinary Bottles game, launcher or registry data is modified.

#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
use intimatr::bottles::{BottleInventory, parse_bottle_names, parse_registered_programs};

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("intimatr-bottles is a native Linux host utility");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(message) = run() {
        eprintln!("intimatr-bottles: {message}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), String> {
    use intimatr::bottles_selection::SelectionRules;

    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_default();
    if action == "--help" || action == "-h" {
        println!(
            "Usage:\n  intimatr-bottles scan\n  intimatr-bottles plan (--all | --include-bottle GLOB | --include-program GLOB) [--exclude-bottle GLOB] [--exclude-program GLOB]\n  intimatr-bottles probe-attach --pid PID --prefix WINEPREFIX --helper PATH [--runner WINE]\n  intimatr-bottles profile-check --config TOML --bottle NAME --exe WINDOWS_PATH\n  intimatr-bottles processes --prefix INITIALIZED_WINEPREFIX\n  intimatr-bottles win-processes --prefix INITIALIZED_WINEPREFIX --helper WINE_PROCESS_LIST_EXE [--runner WINE]\n\nscan and plan are read-only. probe-attach runs only the strictly allowlisted offline Wine test helper; it cannot attach a real game."
        );
        return Ok(());
    }
    if action == "probe-attach" {
        return run_probe_attach(args.collect());
    }
    if action == "profile-check" {
        return run_profile_check(args.collect());
    }
    if action == "processes" {
        return run_prefix_processes(args.collect());
    }
    if action == "win-processes" {
        return run_windows_processes(args.collect());
    }
    if action != "scan" && action != "plan" {
        return Err(
            "expected scan, plan, profile-check, processes, win-processes or probe-attach (use --help)".to_owned(),
        );
    }

    let mut rules = SelectionRules::default();
    if action == "scan" && args.len() > 0 {
        return Err("scan does not accept filters; use plan".to_owned());
    }
    while let Some(flag) = args.next() {
        if action != "plan" {
            return Err("unexpected argument for scan".to_owned());
        }
        if flag == "--all" {
            rules.all = true;
            continue;
        }
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or_else(|| format!("missing pattern after {flag}"))?;
        match flag.as_str() {
            "--include-bottle" => rules.include_bottle.push(value),
            "--include-program" => rules.include_program.push(value),
            "--exclude-bottle" => rules.exclude_bottle.push(value),
            "--exclude-program" => rules.exclude_program.push(value),
            _ => return Err(format!("unsupported filter flag: {flag}")),
        }
    }
    if action == "plan" {
        rules.validate().map_err(str::to_owned)?;
    }

    let raw_bottles = bottles_json(&["list", "bottles"])?;
    let names = parse_bottle_names(&raw_bottles)
        .map_err(|error| format!("cannot decode Bottles list: {error}"))?;

    let mut inventory = Vec::new();
    for name in names {
        let program_json = bottles_json(&["programs", "-b", &name])?;
        let programs = parse_registered_programs(&program_json)
            .map_err(|error| format!("cannot decode programs in bottle {name:?}: {error}"))?;
        inventory.push(BottleInventory {
            bottle: name,
            programs,
        });
    }

    let output = if action == "plan" {
        let plan = rules.plan(&inventory).map_err(str::to_owned)?;
        serde_json::to_string_pretty(&plan)
    } else {
        serde_json::to_string_pretty(&inventory)
    }
    .map_err(|error| format!("cannot encode inventory: {error}"))?;
    println!("{output}");
    eprintln!("Read-only inspection: no game or bottle was modified.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn bottles_json(args: &[&str]) -> Result<String, String> {
    // Invoke the installed native CLI directly, never through a shell.
    let result = Command::new("bottles-cli")
        .arg("--json")
        .args(args)
        .output()
        .map_err(|error| format!("cannot execute native bottles-cli: {error}"))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let short_error: String = stderr.chars().take(400).collect();
        return Err(format!(
            "bottles-cli command failed (exit {:?}): {short_error}",
            result.status.code()
        ));
    }
    if result.stdout.len() > 16 * 1024 * 1024 {
        return Err("bottles-cli JSON result exceeds 16 MiB safety limit".to_owned());
    }
    String::from_utf8(result.stdout)
        .map_err(|error| format!("bottles-cli returned non-UTF-8 JSON: {error}"))
}

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
        Ok(Self {
            pid: pid.ok_or("missing --pid")?,
            prefix: prefix.ok_or("missing --prefix")?,
            helper: helper.ok_or("missing --helper")?,
            runner: runner.unwrap_or_else(|| "wine".to_owned()),
        })
    }
}

#[cfg(target_os = "linux")]
fn run_probe_attach(args: Vec<String>) -> Result<(), String> {
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
    let result = Command::new(&opts.runner)
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
            args(&["--pid", "0", "--prefix", "/tmp", "--helper", "test.exe"]),
            args(&[
                "--pid", "garbage", "--prefix", "/tmp", "--helper", "test.exe",
            ]),
            args(&["--pid", "3", "--prefix", "/tmp"]),
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
        ]))
        .expect("valid test request");
        assert_eq!(parsed.pid, 32);
        assert_eq!(parsed.prefix, "/tmp/owned prefix");
        assert_eq!(parsed.runner, "wine");
    }
}

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
fn run_profile_check(args: Vec<String>) -> Result<(), String> {
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

#[cfg(target_os = "linux")]
fn run_prefix_processes(args: Vec<String>) -> Result<(), String> {
    use intimatr::bottles_processes::inspect_prefix_processes;
    use std::path::Path;

    if args.len() != 2 || args[0] != "--prefix" || args[1].is_empty() {
        return Err("usage: intimatr-bottles processes --prefix INITIALIZED_WINEPREFIX".into());
    }
    let report = inspect_prefix_processes(Path::new(&args[1]), Path::new("/proc"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("cannot encode process report: {error}"))?
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod process_command_tests {
    use super::run_prefix_processes;

    #[test]
    fn rejects_missing_and_unexpected_process_discovery_flags() {
        for bad in [
            Vec::new(),
            vec!["--prefix".to_owned()],
            vec!["--wrong".to_owned(), "/tmp".to_owned()],
            vec![
                "--prefix".to_owned(),
                "/tmp".to_owned(),
                "--unsafe".to_owned(),
            ],
        ] {
            assert!(run_prefix_processes(bad).is_err());
        }
    }
}


#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
struct WindowsProcessesOptions {
    prefix: String,
    helper: String,
    runner: String,
}

#[cfg(target_os = "linux")]
impl WindowsProcessesOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut prefix = None;
        let mut helper = None;
        let mut runner = None;
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--prefix" if prefix.is_none() => prefix = Some(value.clone()),
                "--helper" if helper.is_none() => helper = Some(value.clone()),
                "--runner" if runner.is_none() => runner = Some(value.clone()),
                _ => return Err(format!("unsupported or repeated option {flag}")),
            }
        }
        Ok(Self {
            prefix: prefix.ok_or("missing --prefix")?,
            helper: helper.ok_or("missing --helper")?,
            runner: runner.unwrap_or_else(|| "wine".into()),
        })
    }
}

#[cfg(target_os = "linux")]
fn run_windows_processes(args: Vec<String>) -> Result<(), String> {
    let opts = WindowsProcessesOptions::parse(&args)?;
    let prefix = std::fs::canonicalize(&opts.prefix)
        .map_err(|error| format!("cannot resolve Wine prefix: {error}"))?;
    if !prefix.join("drive_c").is_dir() {
        return Err("Wine prefix must be initialized with drive_c".into());
    }
    let helper = std::fs::canonicalize(&opts.helper)
        .map_err(|error| format!("cannot locate read-only Windows probe: {error}"))?;
    if !helper.is_file()
        || !helper
            .file_name()
            .is_some_and(|name| name == "wine-process-list.exe")
    {
        return Err("expected exactly wine-process-list.exe".into());
    }
    let result = Command::new(&opts.runner)
        .arg(helper)
        .env("WINEPREFIX", &prefix)
        .env("WINEDEBUG", "-all")
        .output()
        .map_err(|error| format!("cannot launch read-only Wine probe: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "Wine process probe exited {:?}: {}",
            result.status.code(),
            String::from_utf8_lossy(&result.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        ));
    }
    if result.stdout.len() > 8 * 1024 * 1024 {
        return Err("Wine process list exceeds 8 MiB safety limit".into());
    }
    let result: serde_json::Value = serde_json::from_slice(&result.stdout)
        .map_err(|error| format!("invalid Windows process list JSON: {error}"))?;
    if result["operation"] != "read_only_wine_windows_processes"
        || result["attachment_attempted"] != false
        || !result["processes"].is_array()
    {
        return Err("unrecognized or unsafe Wine process probe response".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result)
            .map_err(|error| format!("cannot render Windows process list: {error}"))?
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod windows_processes_tests {
    use super::WindowsProcessesOptions;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn requires_explicit_prefix_and_helper_and_rejects_extra_flags() {
        for invalid in [
            args(&[]),
            args(&["--prefix", "/tmp"]),
            args(&["--helper", "wine-process-list.exe"]),
            args(&["--prefix", "/tmp", "--helper", "test.exe", "--unknown", "yes"]),
            args(&["--prefix", "/tmp", "--helper", "test.exe", "--prefix", "/another"]),
        ] {
            assert!(WindowsProcessesOptions::parse(&invalid).is_err());
        }
    }

    #[test]
    fn supports_spaces_in_runner_paths_without_shell() {
        let parsed = WindowsProcessesOptions::parse(&args(&[
            "--prefix", "/tmp/Wine prefix",
            "--helper", "/tmp/wine-process-list.exe",
            "--runner", "/home/user/wine patched/bin/wine",
        ]))
        .unwrap();
        assert_eq!(parsed.runner, "/home/user/wine patched/bin/wine");
        assert_eq!(parsed.prefix, "/tmp/Wine prefix");
    }
}
