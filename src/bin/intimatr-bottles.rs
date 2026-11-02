//! Read-only Bottles inventory CLI for the Linux host.
//!
//! No game, bottle, registry entry, or launcher setting is modified.
//! Future apply/attach functionality must be opt-in and implemented behind
//! a separate, auditable plan/confirmation boundary.

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
            "Usage:\n  intimatr-bottles scan\n  intimatr-bottles plan (--all | --include-bottle GLOB | --include-program GLOB) [--exclude-bottle GLOB] [--exclude-program GLOB]\n\nAll commands are read-only. A plan never installs, loads, or attaches a DLL."
        );
        return Ok(());
    }
    if action != "scan" && action != "plan" {
        return Err("expected scan or plan (use --help)".to_owned());
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
