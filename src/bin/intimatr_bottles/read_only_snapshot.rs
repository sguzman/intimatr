//! Pure snapshot-to-registered-program matching CLI.

#[cfg(target_os = "linux")]
pub(super) fn run_match_snapshot(args: Vec<String>) -> Result<(), String> {
    use intimatr::{
        bottles::{BottleInventory, parse_registered_programs},
        bottles_live_matches::{WindowsProcessSnapshot, match_registered_programs},
    };

    let mut bottle = None;
    let mut programs = None;
    let mut windows = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .ok_or_else(|| format!("missing value after {flag}"))?;
        match flag.as_str() {
            "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
            "--programs" if programs.is_none() => programs = Some(value.clone()),
            "--windows" if windows.is_none() => windows = Some(value.clone()),
            _ => return Err(format!("duplicate or unsupported match option: {flag}")),
        }
    }
    let bottle: String = bottle.ok_or("missing --bottle")?;
    if bottle.trim().is_empty() {
        return Err("bottle name must not be empty".into());
    }
    let programs: String = programs.ok_or("missing --programs")?;
    let windows: String = windows.ok_or("missing --windows")?;

    fn read_bounded(path: &str) -> Result<String, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("cannot inspect JSON fixture: {error}"))?;
        if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
            return Err("expected a regular JSON file of at most 8 MiB".into());
        }
        std::fs::read_to_string(path).map_err(|error| format!("cannot read JSON fixture: {error}"))
    }

    let programs = parse_registered_programs(&read_bounded(&programs)?)
        .map_err(|error| format!("invalid registered Bottles programs: {error}"))?;
    let snapshot = WindowsProcessSnapshot::parse(&read_bounded(&windows)?)?;
    let result = match_registered_programs(&BottleInventory { bottle, programs }, &snapshot);
    println!(
        "{}",
        serde_json::to_string_pretty(&result)
            .map_err(|error| format!("cannot render read-only match report: {error}"))?
    );
    Ok(())
}

