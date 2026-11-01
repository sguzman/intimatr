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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: intimatr-bottles scan\nRead-only JSON inventory of Bottles-registered programs.");
        return Ok(());
    }
    if args != ["scan"] {
        return Err("usage: intimatr-bottles scan (read-only)".to_owned());
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

    let result = serde_json::to_string_pretty(&inventory)
        .map_err(|error| format!("cannot encode inventory: {error}"))?;
    println!("{result}");
    eprintln!("Intimatr inventory is read-only and lists registered programs only.");
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
