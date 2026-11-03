//! Read-only, selected-Bottles-prefix compatibility inventory.
//! Uses native Bottles JSON and safe C: file translation only; never Wine.
//! No program name, prefix path or static import count grants attach rights.

use std::{
    fs,
    path::{Path, PathBuf},
};

use intimatr::{
    bottles::{RegisteredProgram, parse_bottle_names, parse_registered_programs},
    bottles_prefix::{parse_bottles_root, verify_bottle_prefix},
    pe_timer_imports::inspect_pe_timer_imports,
};

use super::{bottles_json_with_command, pe_compat::classify_imports};

const MAX_REGISTERED_PROGRAMS: usize = 1024;

struct Options {
    bottle: String,
    cli: String,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut bottle = None;
    let mut cli = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| format!("missing bottle-compat value after {flag}"))?;
        match flag.as_str() {
            "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
            "--bottles-cli" if cli.is_none() => cli = Some(value.clone()),
            _ => return Err(format!("duplicate or unexpected bottle-compat flag: {flag}")),
        }
    }
    let bottle = bottle.ok_or("bottle-compat requires exactly one --bottle NAME")?;
    if bottle.trim().is_empty() {
        return Err("bottle-compat requires a nonempty bottle name".into());
    }
    let cli = cli.unwrap_or_else(|| "bottles-cli".to_owned());
    if Path::new(&cli)
        .file_name()
        .is_none_or(|file| file != "bottles-cli")
    {
        return Err("expected a native bottles-cli executable".into());
    }
    Ok(Options { bottle, cli })
}

/// Do not guess Wine drive mappings. Restrict the initial scanner to C:
/// inside this bottle's actual drive_c; Z:, UNC, shell links, relative and
/// traversal paths are indeterminate. Never fall through to host root.
fn resolve_c_drive_file(prefix: &Path, windows_path: &str) -> Result<PathBuf, String> {
    let slashes = windows_path.replace('\\', "/");
    let Some(rest) = slashes
        .strip_prefix("C:/")
        .or_else(|| slashes.strip_prefix("c:/"))
    else {
        return Err("not a C: absolute path in the verified bottle".into());
    };
    let mut relative = PathBuf::new();
    let mut count = 0_usize;
    for part in rest.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.contains(':')
            || part.chars().any(char::is_control)
        {
            return Err("unsafe Windows path component or traversal".into());
        }
        relative.push(part);
        count += 1;
        if count > 128 {
            return Err("Windows executable path exceeds bounded component count".into());
        }
    }
    if count == 0 {
        return Err("no executable path below C: drive".into());
    }
    let root = fs::canonicalize(prefix.join("drive_c"))
        .map_err(|error| format!("cannot resolve selected bottle C: drive: {error}"))?;
    if !root.starts_with(prefix) {
        return Err("selected bottle C: drive escapes its verified prefix".into());
    }
    let selected = fs::canonicalize(root.join(relative))
        .map_err(|error| format!("registered executable is unavailable: {error}"))?;
    if !selected.starts_with(&root) {
        return Err("registered executable resolves outside selected bottle C: drive".into());
    }
    if !selected.is_file() {
        return Err("registered executable is not a regular file".into());
    }
    Ok(selected)
}

fn inspect_program(prefix: &Path, program: &RegisteredProgram) -> serde_json::Value {
    let (resolved, inspection) = match resolve_c_drive_file(prefix, &program.path) {
        Ok(file) => {
            let result = inspect_pe_timer_imports(&file);
            (Some(file), result)
        }
        Err(error) => (None, Err(error)),
    };
    serde_json::json!({
        "registered_name": program.name,
        "registered_executable": program.executable,
        "registered_windows_path": program.path,
        "resolved_linux_file": resolved,
        "static_pe_compatibility": classify_imports(inspection),
    })
}

pub(super) fn run_bottle_compat(args: Vec<String>) -> Result<(), String> {
    let options = parse_options(&args)?;
    let names = parse_bottle_names(&bottles_json_with_command(
        &options.cli,
        &["list", "bottles"],
    )?)
    .map_err(|error| format!("invalid native Bottles inventory: {error}"))?;
    if !names.contains(&options.bottle) {
        return Err("selected bottle not found in native Bottles inventory".into());
    }
    let root = bottles_json_with_command(&options.cli, &["info", "bottles-path"])?;
    let prefix = parse_bottles_root(&root)?.join(&options.bottle);
    let ownership = verify_bottle_prefix(&root, &options.bottle, &prefix)?;
    let programs = parse_registered_programs(&bottles_json_with_command(
        &options.cli,
        &["programs", "-b", &options.bottle],
    )?)
    .map_err(|error| format!("invalid registered Bottles programs: {error}"))?;
    if programs.len() > MAX_REGISTERED_PROGRAMS {
        return Err("selected bottle has too many programs for bounded PE inspection".into());
    }
    let reviewed = programs
        .iter()
        .map(|program| inspect_program(&ownership.canonical_bottle_prefix, program))
        .collect::<Vec<_>>();
    let output = serde_json::json!({
        "operation": "read_only_registered_bottle_static_pe_compatibility",
        "selected_bottle": options.bottle,
        "verified_bottle_prefix": ownership.canonical_bottle_prefix,
        "bottle_prefix_association_verified": true,
        "registered_programs_only": true,
        "wine_runner_verified": false,
        "process_identity_verified": false,
        "gameplay_speed_verified": false,
        "process_attachment_attempted": false,
        "timer_hook_installation_attempted": false,
        "programs": reviewed,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("cannot serialize read-only bottle scan: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "intimatr-bottle-compat-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(path.join("drive_c/Games")).unwrap();
            fs::write(path.join("drive_c/Games/Game.exe"), b"invalid PE").unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rejects_implicit_bottles_and_misnamed_cli() {
        for args in [
            vec![],
            vec!["--bottle".into(), "".into()],
            vec!["--bottle".into(), "Games".into(), "--bottle".into(), "Games".into()],
            vec!["--bottle".into(), "Games".into(), "--bottles-cli".into(), "sh".into()],
        ] {
            assert!(parse_options(&args).is_err());
        }
    }

    #[test]
    fn c_drive_mapping_requires_file_inside_selected_prefix() {
        let f = Fixture::new();
        let selected = resolve_c_drive_file(&f.0, r"C:\Games\Game.exe").unwrap();
        assert!(selected.starts_with(&f.0));
        assert!(resolve_c_drive_file(&f.0, r"c:/Games/Game.exe").is_ok());
        for invalid in [
            r"Z:\Games\Game.exe",
            r"C:\..\Games\Game.exe",
            r"C:\Games\..\Game.exe",
            r"C:\Games\\Game.exe",
            r"C:Games\Game.exe",
            r"\\server\share\Game.exe",
            r"C:\Games\Missing.exe",
        ] {
            assert!(resolve_c_drive_file(&f.0, invalid).is_err(), "{invalid}");
        }
        let item = RegisteredProgram {
            name: "Game".into(),
            executable: "Game.exe".into(),
            path: r"C:\Games\Game.exe".into(),
            arguments: String::new(),
        };
        let report = inspect_program(&f.0, &item);
        assert_eq!(
            report["static_pe_compatibility"]["static_import_status"],
            "static_import_inventory_unknown"
        );
        assert_eq!(
            report["static_pe_compatibility"]["process_attachment_attempted"],
            false
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_registered_game_cannot_escape_bottle() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let outside = f.0.parent().unwrap().join(format!(
            "intimatr-outside-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&outside, b"not owned").unwrap();
        symlink(&outside, f.0.join("drive_c/Games/Escape.exe")).unwrap();
        assert!(resolve_c_drive_file(&f.0, r"C:\Games\Escape.exe").is_err());
        fs::remove_file(&outside).unwrap();
    }
}
