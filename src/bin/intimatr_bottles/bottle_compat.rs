//! Read-only, selected-Bottles-prefix compatibility inventory.
//! Uses native Bottles JSON and safe C: file translation only; never Wine.
//! No program name, prefix path or static import count grants attach rights.

use std::{
    fs,
    path::{Path, PathBuf},
};

use intimatr::{
    bottles::{RegisteredProgram, parse_bottle_names},
    bottles_prefix::{parse_bottles_root, verify_bottle_prefix},
    pe_timer_imports::inspect_pe_timer_imports,
};

use super::{bottles_json_with_command, pe_compat::classify_imports};
use serde_json::Value;

const MAX_REGISTERED_PROGRAMS: usize = 1024;
const MAX_DIRECTORY_ENTRIES: usize = 8192;

struct Options {
    bottle: Option<String>,
    all: bool,
    cli: String,
    include_mapped_drives: bool,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut bottle = None;
    let mut cli = None;
    let mut all = false;
    let mut include_mapped_drives = false;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        if flag == "--all" {
            if all {
                return Err("duplicate --all selection".into());
            }
            all = true;
            continue;
        }
        if flag == "--include-mapped-drives" {
            if include_mapped_drives {
                return Err("duplicate --include-mapped-drives acknowledgment".into());
            }
            include_mapped_drives = true;
            continue;
        }
        let value = iter
            .next()
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| format!("missing bottle-compat value after {flag}"))?;
        match flag.as_str() {
            "--bottle" if bottle.is_none() => bottle = Some(value.clone()),
            "--bottles-cli" if cli.is_none() => cli = Some(value.clone()),
            _ => {
                return Err(format!(
                    "duplicate or unexpected bottle-compat flag: {flag}"
                ));
            }
        }
    }
    if bottle
        .as_deref()
        .is_some_and(|name: &str| name.trim().is_empty())
    {
        return Err("bottle-compat requires a nonempty bottle name".into());
    }
    if all == bottle.is_some() {
        return Err("bottle-compat requires exactly one --bottle NAME or --all".into());
    }
    let cli = cli.unwrap_or_else(|| "bottles-cli".to_owned());
    if Path::new(&cli)
        .file_name()
        .is_none_or(|file| file != "bottles-cli")
    {
        return Err("expected a native bottles-cli executable".into());
    }
    Ok(Options {
        bottle,
        all,
        cli,
        include_mapped_drives,
    })
}

/// One case-insensitive lookup component at a time. On case-sensitive
/// Linux filesystems, distinct names may fold to the same Windows filename:
/// reject both rather than guessing which executable Wine would open.
/// We only fold ASCII; for Unicode the name must match exactly, avoiding
/// unsupported assumptions about Windows Unicode casing rules.
fn resolve_windows_component(directory: &Path, requested: &str) -> Result<PathBuf, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot inspect registered executable directory: {error}"))?;
    let mut chosen = None;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            return Err("registered executable directory exceeds bounded entry count".into());
        }
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == requested
            || (name.is_ascii() && requested.is_ascii() && name.eq_ignore_ascii_case(requested))
        {
            if chosen.is_some() {
                return Err("ambiguous case-insensitive registered executable path".into());
            }
            chosen = Some(entry.path());
        }
    }
    chosen.ok_or_else(|| format!("registered executable component is unavailable: {requested}"))
}

/// Every component is checked before reading further. Even for an
/// explicitly selected host-mapped drive, paths cannot escape that drive's
/// canonical root. ASCII case collisions remain unresolvable.
fn resolve_components(root: &Path, rest: &str) -> Result<PathBuf, String> {
    let mut parts = Vec::new();
    for part in rest.split('/') {
        if part.is_empty()
            || matches!(part, "." | "..")
            || part.contains(':')
            || part.chars().any(char::is_control)
        {
            return Err("unsafe Windows path component or traversal".into());
        }
        parts.push(part);
        if parts.len() > 128 {
            return Err("Windows executable path exceeds bounded component count".into());
        }
    }
    if parts.is_empty() {
        return Err("no executable path below selected mapped drive".into());
    }
    let mut current = root.to_path_buf();
    for part in parts {
        let next = resolve_windows_component(&current, part)?;
        current = fs::canonicalize(&next)
            .map_err(|error| format!("cannot resolve registered executable path: {error}"))?;
        if !current.starts_with(root) {
            return Err("registered executable resolves outside selected mapped drive".into());
        }
    }
    if !current.is_file() {
        return Err("registered executable is not a regular file".into());
    }
    Ok(current)
}

/// The default inventory never traverses another host drive. Preserve
/// the verified prefix's ordinary C: containment even if drive_c is a link.
fn resolve_c_drive_file(prefix: &Path, windows_path: &str) -> Result<PathBuf, String> {
    let slashes = windows_path.replace('\\', "/");
    let Some(rest) = slashes
        .strip_prefix("C:/")
        .or_else(|| slashes.strip_prefix("c:/"))
    else {
        return Err("not a C: absolute path in the verified bottle".into());
    };
    let root = fs::canonicalize(prefix.join("drive_c"))
        .map_err(|error| format!("cannot resolve selected bottle C: drive: {error}"))?;
    if !root.starts_with(prefix) {
        return Err("selected bottle C: drive escapes its verified prefix".into());
    }
    resolve_components(&root, rest)
}

/// Only an explicit opt-in allows reading an external registered file.
/// Wine maps DOS drives through symlinks in this exact prefix's dosdevices.
/// The mapping itself must be an existing symlink to a directory; never
/// infer Z: as host root or substitute an unverified fallback.
pub(super) fn resolve_registered_file(
    prefix: &Path,
    windows_path: &str,
    include_mapped_drives: bool,
) -> Result<PathBuf, String> {
    let slashes = windows_path.replace('\\', "/");
    let (drive, rest) = slashes
        .split_once(":/")
        .ok_or("registered executable has no absolute DOS drive path")?;
    if drive.len() != 1 || !drive.as_bytes()[0].is_ascii_alphabetic() {
        return Err("unsupported registered executable drive letter".into());
    }
    if drive.eq_ignore_ascii_case("c") {
        return resolve_c_drive_file(prefix, windows_path);
    }
    if !include_mapped_drives {
        return Err("non-C: registered drive requires explicit --include-mapped-drives".into());
    }
    let mapping = prefix
        .join("dosdevices")
        .join(format!("{}:", drive.to_ascii_lowercase()));
    let metadata = fs::symlink_metadata(&mapping)
        .map_err(|error| format!("selected drive has no verified Wine mapping: {error}"))?;
    if !metadata.file_type().is_symlink() {
        return Err("selected Wine drive mapping is not a symlink".into());
    }
    let root = fs::canonicalize(&mapping)
        .map_err(|error| format!("cannot resolve selected Wine drive mapping: {error}"))?;
    if !root.is_dir() {
        return Err("selected Wine drive mapping is not a directory".into());
    }
    resolve_components(&root, rest)
}

fn inspect_program(
    prefix: &Path,
    program: &RegisteredProgram,
    include_mapped_drives: bool,
) -> serde_json::Value {
    let (resolved, inspection) =
        match resolve_registered_file(prefix, &program.path, include_mapped_drives) {
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
        "registration_metadata_valid": true,
        "resolved_linux_file": resolved,
        "static_pe_compatibility": classify_imports(inspection),
    })
}

/// Preserve valid neighbors when a registered program has null metadata.
/// Only this read-only inventory tolerates malformed individual records.
fn inspect_program_inventory(
    prefix: &Path,
    raw: &str,
    include_mapped_drives: bool,
) -> Result<Vec<Value>, String> {
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|error| format!("invalid native Bottles programs JSON: {error}"))?;
    let entries = parsed
        .as_array()
        .ok_or("native Bottles registered programs must be an array")?;
    if entries.len() > MAX_REGISTERED_PROGRAMS {
        return Err("selected bottle has too many programs for bounded PE inspection".into());
    }
    Ok(entries
        .iter()
        .enumerate()
        .map(
            |(index, record)| match serde_json::from_value::<RegisteredProgram>(record.clone()) {
                Ok(program)
                    if !program.name.trim().is_empty()
                        && !program.executable.trim().is_empty()
                        && !program.path.trim().is_empty() =>
                {
                    inspect_program(prefix, &program, include_mapped_drives)
                }
                result => {
                    let reason = match result {
                        Ok(_) => "required registered-program identity field is empty".to_owned(),
                        Err(error) => error.to_string(),
                    };
                    serde_json::json!({
                        "registered_index": index,
                        "registered_name": record.get("name").and_then(Value::as_str),
                        "registered_executable": record.get("executable").and_then(Value::as_str),
                        "registered_windows_path": record.get("path").and_then(Value::as_str),
                        "registration_metadata_valid": false,
                        "resolved_linux_file": null,
                        "static_pe_compatibility": classify_imports(Err(format!(
                            "invalid registered program entry {index}: {reason}"
                        ))),
                    })
                }
            },
        )
        .collect())
}

/// Inspect only registered executable files within one verified bottle.
fn inspect_selected_bottle(
    root: &str,
    cli: &str,
    bottle: &str,
    include_mapped_drives: bool,
) -> Result<(PathBuf, Vec<serde_json::Value>), String> {
    let prefix = parse_bottles_root(root)?.join(bottle);
    let ownership = verify_bottle_prefix(root, bottle, &prefix)?;
    let reviewed = inspect_program_inventory(
        &ownership.canonical_bottle_prefix,
        &bottles_json_with_command(cli, &["programs", "-b", bottle])?,
        include_mapped_drives,
    )?;
    Ok((ownership.canonical_bottle_prefix, reviewed))
}

pub(super) fn run_bottle_compat(args: Vec<String>) -> Result<(), String> {
    let options = parse_options(&args)?;
    let names = parse_bottle_names(&bottles_json_with_command(
        &options.cli,
        &["list", "bottles"],
    )?)
    .map_err(|error| format!("invalid native Bottles inventory: {error}"))?;
    if !options.all
        && !names
            .iter()
            .any(|name| Some(name.as_str()) == options.bottle.as_deref())
    {
        return Err("selected bottle not found in native Bottles inventory".into());
    }
    let root = bottles_json_with_command(&options.cli, &["info", "bottles-path"])?;

    if options.all {
        if names.len() > 256 {
            return Err("all-bottle compatibility scan exceeds 256 registered bottles".into());
        }
        let mut reports = Vec::new();
        let mut four_static_names = 0_usize;
        let mut incomplete_static_names = 0_usize;
        let mut unknown_static_imports = 0_usize;
        let mut uninspected_bottles = 0_usize;
        for bottle in names {
            match inspect_selected_bottle(
                &root,
                &options.cli,
                &bottle,
                options.include_mapped_drives,
            ) {
                Ok((prefix, programs)) => {
                    for program in &programs {
                        match program["static_pe_compatibility"]["static_import_status"].as_str() {
                            Some("static_four_timer_names_present") => four_static_names += 1,
                            Some("static_timer_imports_missing") => incomplete_static_names += 1,
                            _ => unknown_static_imports += 1,
                        }
                    }
                    reports.push(serde_json::json!({
                        "bottle": bottle,
                        "bottle_prefix_association_verified": true,
                        "verified_bottle_prefix": prefix,
                        "inspection_error": null,
                        "programs": programs,
                    }));
                }
                Err(error) => {
                    uninspected_bottles += 1;
                    reports.push(serde_json::json!({
                        "bottle": bottle,
                        "bottle_prefix_association_verified": false,
                        "verified_bottle_prefix": null,
                        "inspection_error": error,
                        "programs": null,
                    }));
                }
            }
        }
        let output = serde_json::json!({
            "operation": "read_only_all_registered_bottles_static_pe_compatibility",
            "registered_bottles_only": true,
            "registered_bottle_count": reports.len(),
            "bottles_with_unresolved_inventory": uninspected_bottles,
            "programs_with_all_four_static_names": four_static_names,
            "programs_with_incomplete_static_names": incomplete_static_names,
            "programs_with_unknown_static_imports": unknown_static_imports,
            "external_drive_inspection_opted_in": options.include_mapped_drives,
            "wine_runner_verified": false,
            "process_identity_verified": false,
            "gameplay_speed_verified": false,
            "process_attachment_attempted": false,
            "timer_hook_installation_attempted": false,
            "bottles": reports,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .map_err(|error| format!("cannot serialize all-bottle scan: {error}"))?
        );
        return Ok(());
    }

    let bottle = options.bottle.as_deref().ok_or("missing selected bottle")?;
    let (prefix, reviewed) =
        inspect_selected_bottle(&root, &options.cli, bottle, options.include_mapped_drives)?;
    let output = serde_json::json!({
        "operation": "read_only_registered_bottle_static_pe_compatibility",
        "selected_bottle": bottle,
        "verified_bottle_prefix": prefix,
        "bottle_prefix_association_verified": true,
        "registered_programs_only": true,
        "external_drive_inspection_opted_in": options.include_mapped_drives,
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
            vec!["--all".into(), "--all".into()],
            vec!["--all".into(), "--bottle".into(), "Games".into()],
            vec![
                "--bottle".into(),
                "Games".into(),
                "--bottle".into(),
                "Games".into(),
            ],
            vec![
                "--bottle".into(),
                "Games".into(),
                "--bottles-cli".into(),
                "sh".into(),
            ],
            vec![
                "--bottle".into(),
                "Games".into(),
                "--include-mapped-drives".into(),
                "--include-mapped-drives".into(),
            ],
        ] {
            assert!(parse_options(&args).is_err());
        }
    }

    #[test]
    fn all_bottles_selection_is_explicit_and_exclusive() {
        let all = parse_options(&["--all".to_owned()]).unwrap();
        assert!(all.all);
        assert!(all.bottle.is_none());
        assert!(!all.include_mapped_drives);
        assert!(parse_options(&[]).is_err());
        assert!(parse_options(&["--all".to_owned(), "--all".to_owned()]).is_err());
        assert!(
            parse_options(&[
                "--all".to_owned(),
                "--bottle".to_owned(),
                "Games".to_owned(),
            ])
            .is_err()
        );
        let one = parse_options(&["--bottle".to_owned(), "Games".to_owned()]).unwrap();
        assert_eq!(one.bottle.as_deref(), Some("Games"));
        assert!(!one.all);
    }

    #[test]
    fn c_drive_mapping_requires_file_inside_selected_prefix() {
        let f = Fixture::new();
        let selected = resolve_c_drive_file(&f.0, r"C:\Games\Game.exe").unwrap();
        assert!(selected.starts_with(&f.0));
        assert!(resolve_c_drive_file(&f.0, r"c:/Games/Game.exe").is_ok());
        assert_eq!(
            resolve_c_drive_file(&f.0, r"c:\gAmEs\gAME.ExE").unwrap(),
            selected
        );
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
        let report = inspect_program(&f.0, &item, false);
        assert_eq!(
            report["static_pe_compatibility"]["static_import_status"],
            "static_import_inventory_unknown"
        );
        assert_eq!(
            report["static_pe_compatibility"]["process_attachment_attempted"],
            false
        );
    }

    #[test]
    fn invalid_registration_does_not_mask_valid_neighbor() {
        let f = Fixture::new();
        let raw = serde_json::json!([
            {"name":"Valid","executable":"Game.exe","path":r"C:\Games\Game.exe"},
            {"name":"Missing Path","executable":"Missing.exe","path":null},
            {"name":"Null Arguments","executable":"Game.exe","path":r"C:\Games\Game.exe","arguments":null}
        ]).to_string();
        let result = inspect_program_inventory(&f.0, &raw, false).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0]["registration_metadata_valid"], true);
        assert_eq!(result[1]["registered_name"], "Missing Path");
        assert_eq!(result[1]["registration_metadata_valid"], false);
        assert_eq!(
            result[1]["static_pe_compatibility"]["static_import_status"],
            "static_import_inventory_unknown"
        );
        assert_eq!(result[2]["registration_metadata_valid"], false);
        assert_eq!(
            result[2]["static_pe_compatibility"]["process_attachment_attempted"],
            false
        );
        assert!(inspect_program_inventory(&f.0, "{}", false).is_err());
    }

    #[test]
    fn ambiguous_windows_case_is_unknown_even_with_exact_linux_spelling() {
        let f = Fixture::new();
        fs::write(f.0.join("drive_c/Games/GAME.EXE"), b"different file").unwrap();
        for path in [r"C:\Games\Game.exe", r"c:\games\game.exe"] {
            assert!(
                resolve_c_drive_file(&f.0, path)
                    .unwrap_err()
                    .contains("ambiguous case-insensitive"),
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn mapped_external_drive_needs_opt_in_and_verified_symlink() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let external = f.0.with_extension("external");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("Game.exe"), b"not a PE").unwrap();
        fs::create_dir_all(f.0.join("dosdevices")).unwrap();
        symlink(&external, f.0.join("dosdevices/d:")).unwrap();

        assert!(
            resolve_registered_file(&f.0, r"D:\Game.exe", false)
                .unwrap_err()
                .contains("--include-mapped-drives")
        );
        assert_eq!(
            resolve_registered_file(&f.0, r"d:\game.EXE", true).unwrap(),
            fs::canonicalize(external.join("Game.exe")).unwrap()
        );
        assert!(resolve_registered_file(&f.0, r"E:\Game.exe", true).is_err());
        assert!(resolve_registered_file(&f.0, r"D:\..\Game.exe", true).is_err());
        assert!(resolve_registered_file(&f.0, r"\\server\Game.exe", true).is_err());

        let escaped = external.join("Outside.exe");
        let outside = f.0.join("drive_c/Games/Game.exe");
        symlink(&outside, &escaped).unwrap();
        assert!(
            resolve_registered_file(&f.0, r"D:\Outside.exe", true)
                .unwrap_err()
                .contains("outside selected mapped drive")
        );

        fs::remove_dir_all(&external).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_symlinked_external_drive_mapping() {
        let f = Fixture::new();
        fs::create_dir_all(f.0.join("dosdevices/d:")).unwrap();
        assert!(
            resolve_registered_file(&f.0, r"D:\Game.exe", true)
                .unwrap_err()
                .contains("not a symlink")
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
