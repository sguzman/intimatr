//! Explicit, reversible inherited-environment allowlist changes for selected
//! native Bottles prefixes. No Wine launch, DLL install, or graphics hooks.
//! A narrow, lossless editor supports Bottles' ordinary block-list YAML only;
//! unusual YAML fails closed instead of being rewritten through a lossy parser.

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use intimatr::{
    bottles::parse_bottle_names,
    bottles_prefix::{parse_bottles_root, verify_bottle_prefix},
    speed_profile_fingerprint::{sha256_hex, sha256_words},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::bottles_json_with_command;

const ORIGINAL: &str = "bottle.yml.intimatr-env-original";
const MARKER: &str = "bottle.yml.intimatr-env-state.json";
const STAGED: &str = "bottle.yml.intimatr-env-staged";
const MAX_CONFIG: usize = 1_048_576;

#[derive(Debug, Clone)]
struct Options {
    action: String,
    bottles: Vec<String>,
    allow: Vec<String>,
}

fn valid_variable(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('A'..='Z') | Some('_'))
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && name.len() <= 128
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let action = args
            .next()
            .ok_or("expected plan, status, apply, or restore")?;
        if !["plan", "status", "apply", "restore"].contains(&action.as_str()) {
            return Err("expected plan, status, apply, or restore".into());
        }
        let (mut bottles, mut allow) = (Vec::new(), Vec::new());
        let mut closed = false;
        while let Some(flag) = args.next() {
            if flag == "--bottles-closed" && !closed {
                closed = true;
                continue;
            }
            let value = args
                .next()
                .filter(|s| !s.is_empty() && !s.starts_with("--"))
                .ok_or_else(|| format!("missing value after {flag}"))?;
            match flag.as_str() {
                "--bottle" => {
                    if value == "." || value == ".." || value.contains('/') || value.contains('\\')
                    {
                        return Err("unsafe bottle name".into());
                    }
                    bottles.push(value);
                }
                "--allow" if action != "restore" => allow.push(value),
                "--preset" if action != "restore" && value == "intimatr-hud" => {
                    allow.extend(
                        [
                            "INTIMATR_SPEED_PROFILE",
                            "INTIMATR_EXPERIMENTAL_STARTUP_TIMERS",
                            "MANGOHUD",
                            "INTIMATR_HUD_TELEMETRY",
                            "MANGOHUD_CONFIGFILE",
                            "LD_PRELOAD",
                        ]
                        .map(str::to_owned),
                    );
                }
                _ => return Err(format!("unsupported flag or value: {flag}")),
            }
        }
        if bottles.is_empty() {
            return Err("select at least one --bottle NAME explicitly".into());
        }
        let mut unique_bottles = BTreeSet::new();
        if bottles
            .iter()
            .any(|name| !unique_bottles.insert(name.clone()))
        {
            return Err("duplicate bottle selection".into());
        }
        if allow.iter().any(|name| !valid_variable(name)) {
            return Err("invalid environment variable name".into());
        }
        allow.sort_unstable();
        allow.dedup();
        if action != "restore" && allow.is_empty() {
            return Err("specify --preset intimatr-hud or at least one --allow NAME".into());
        }
        if matches!(action.as_str(), "apply" | "restore") != closed {
            return Err(
                "apply/restore require --bottles-closed; plan/status must be read-only".into(),
            );
        }
        bottles.sort_unstable();
        Ok(Self {
            action,
            bottles,
            allow,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    version: u32,
    bottle: String,
    original_sha256: String,
    installed_sha256: String,
    added: Vec<String>,
}

fn digest(bytes: &[u8]) -> String {
    sha256_hex(&sha256_words(bytes))
}

fn regular_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_CONFIG as u64 {
        return Err(format!(
            "refusing linked, nonregular, or oversized file: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("refusing to create {}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Identify a plain top-level block list, preserving every unrelated YAML byte.
/// The ordinary Bottles/PyYAML serializer writes "-" items with no indent.
/// An unusual flow list, tag, comment inside the list, or nested node is refused.
fn block_list(config: &str) -> Result<(usize, Vec<String>), String> {
    let mut offset = 0;
    let mut found = false;
    let mut end = 0;
    let mut vars = Vec::new();
    if !config.ends_with('\n') {
        return Err("Bottles YAML lacks terminal newline; refusing lossless patch".into());
    }
    for line in config.split_inclusive('\n') {
        let raw = line.trim_end_matches(&['\r', '\n'][..]);
        if !found {
            if raw == "Inherited_Environment_Variables:" {
                found = true;
                end = offset + line.len();
            }
        } else if raw.starts_with("- ") {
            let item = &raw[2..];
            let unquoted = if item.len() >= 2
                && ((item.starts_with('\'') && item.ends_with('\''))
                    || (item.starts_with('"') && item.ends_with('"')))
            {
                &item[1..item.len() - 1]
            } else {
                item
            };
            if !valid_variable(unquoted) {
                return Err("unusual YAML inherited variable list; refusing patch".into());
            }
            vars.push(unquoted.to_owned());
            end = offset + line.len();
        } else if raw.is_empty() {
            // Blank lines between list and the next YAML key are retained.
        } else if !raw.starts_with(' ') && !raw.starts_with('-') && raw.contains(':') {
            break;
        } else {
            return Err("unsupported inherited-variable YAML layout".into());
        }
        offset += line.len();
    }
    if !found {
        return Err("Bottles YAML lacks a plain inherited-variable block list".into());
    }
    let mut unique = BTreeSet::new();
    if vars.iter().any(|name| !unique.insert(name)) {
        return Err("duplicate inherited variables in Bottles YAML".into());
    }
    Ok((end, vars))
}

fn render(
    config: &str,
    allow: &[String],
    expected: &[String],
) -> Result<(String, Vec<String>), String> {
    let (end, existing) = block_list(config)?;
    if existing != expected {
        return Err("Bottles YAML inherited list differs from native Bottles inventory".into());
    }
    let missing = allow
        .iter()
        .filter(|name| !existing.contains(name))
        .cloned()
        .collect::<Vec<_>>();
    let mut updated = String::with_capacity(config.len() + missing.len() * 32);
    updated.push_str(&config[..end]);
    for name in &missing {
        updated.push_str("- ");
        updated.push_str(name);
        updated.push('\n');
    }
    updated.push_str(&config[end..]);
    Ok((updated, missing))
}

struct Selection {
    name: String,
    config: PathBuf,
    initial: Vec<u8>,
    updated: Vec<u8>,
    missing: Vec<String>,
    marker: PathBuf,
    backup: PathBuf,
    staged: PathBuf,
}

fn prepare(
    name: &str,
    allow: &[String],
    inventory: &Value,
    bottles_root: &str,
) -> Result<Selection, String> {
    let record = inventory
        .get("bottles")
        .or_else(|| inventory.get("Bottles"))
        .unwrap_or(inventory)
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing native Bottles settings for {name}"))?;
    let limited = record
        .get("Limit_System_Environment")
        .and_then(Value::as_bool)
        .ok_or("native Bottles inventory lacks Limit_System_Environment boolean")?;
    let inherited = record
        .get("Inherited_Environment_Variables")
        .and_then(Value::as_array)
        .ok_or("native Bottles inventory lacks inherited environment list")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or("non-string inherited variable")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let overrides = record
        .get("Environment_Variables")
        .and_then(Value::as_object)
        .ok_or("native Bottles inventory lacks explicit environment map")?;
    if allow
        .iter()
        .any(|name| overrides.keys().any(|key| key.eq_ignore_ascii_case(name)))
    {
        return Err(format!(
            "bottle {name} explicitly overrides a requested variable"
        ));
    }
    let root = parse_bottles_root(bottles_root).map_err(|e| e.to_string())?;
    let prefix = root.join(name);
    let identity = verify_bottle_prefix(bottles_root, name, &prefix)
        .map_err(|e| format!("cannot verify selected bottle {name}: {e}"))?;
    let prefix = identity.canonical_bottle_prefix;
    let config = prefix.join("bottle.yml");
    let initial = regular_bytes(&config)?;
    let text = std::str::from_utf8(&initial).map_err(|_| "Bottles YAML is not UTF-8")?;
    let requested: &[String] = if limited { allow.as_slice() } else { &[] };
    let (updated, missing) = render(text, requested, &inherited)?;
    Ok(Selection {
        name: name.to_owned(),
        config,
        initial,
        updated: updated.into_bytes(),
        missing,
        marker: prefix.join(MARKER),
        backup: prefix.join(ORIGINAL),
        staged: prefix.join(STAGED),
    })
}

fn read_state(selected: &Selection) -> Result<Option<State>, String> {
    match fs::symlink_metadata(&selected.marker) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot inspect owned environment marker: {e}")),
        Ok(_) => {
            let bytes = regular_bytes(&selected.marker)?;
            let state: State = serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid owned environment marker: {e}"))?;
            if state.version != 1 || state.bottle != selected.name {
                return Err("environment marker version or bottle identity mismatch".into());
            }
            Ok(Some(state))
        }
    }
}

fn status(selected: &Selection) -> Result<String, String> {
    if let Some(state) = read_state(selected)? {
        let actual = digest(&selected.initial);
        if actual == state.installed_sha256 {
            Ok("owned_unchanged".into())
        } else if actual == state.original_sha256 {
            Ok("interrupted_before_update".into())
        } else {
            Ok("modified_since_intimatr".into())
        }
    } else if fs::symlink_metadata(&selected.backup).is_ok() {
        Err("orphaned Intimatr environment backup; refusing automatic change".into())
    } else {
        Ok("unmanaged".into())
    }
}

fn apply(selected: &Selection) -> Result<(), String> {
    if selected.missing.is_empty() {
        return Ok(());
    }
    if status(selected)? != "unmanaged" {
        return Err("existing or interrupted Intimatr environment edit; restore first".into());
    }
    if fs::symlink_metadata(&selected.staged).is_ok() {
        return Err("interrupted staged environment file found".into());
    }
    if regular_bytes(&selected.config)? != selected.initial {
        return Err("Bottles configuration changed since preflight; refusing write".into());
    }
    let state = State {
        version: 1,
        bottle: selected.name.clone(),
        original_sha256: digest(&selected.initial),
        installed_sha256: digest(&selected.updated),
        added: selected.missing.clone(),
    };
    let marker = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
    write_new(&selected.backup, &selected.initial)?;
    if let Err(err) = write_new(&selected.marker, &marker) {
        let _ = fs::remove_file(&selected.backup);
        return Err(err);
    }
    if let Err(err) = write_new(&selected.staged, &selected.updated) {
        let _ = fs::remove_file(&selected.marker);
        let _ = fs::remove_file(&selected.backup);
        return Err(err);
    }
    let result = (|| -> Result<(), String> {
        let permissions = fs::metadata(&selected.config)
            .map_err(|e| e.to_string())?
            .permissions();
        fs::set_permissions(&selected.staged, permissions).map_err(|e| e.to_string())?;
        if regular_bytes(&selected.config)? != selected.initial {
            return Err("Bottles configuration changed during staging".into());
        }
        fs::rename(&selected.staged, &selected.config)
            .map_err(|e| format!("cannot atomically update Bottles YAML: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&selected.staged);
        let _ = fs::remove_file(&selected.marker);
        let _ = fs::remove_file(&selected.backup);
    }
    result
}

fn restore(selected: &Selection) -> Result<(), String> {
    let state = read_state(selected)?.ok_or("no Intimatr-owned environment change to restore")?;
    let original = regular_bytes(&selected.backup)?;
    if digest(&original) != state.original_sha256 {
        return Err("original environment backup checksum mismatch".into());
    }
    let current_hash = digest(&regular_bytes(&selected.config)?);
    if current_hash != state.installed_sha256 && current_hash != state.original_sha256 {
        return Err(
            "Bottles settings changed after Intimatr apply; refusing destructive restore".into(),
        );
    }
    if current_hash == state.installed_sha256 {
        if fs::symlink_metadata(&selected.staged).is_ok() {
            return Err("stale Intimatr environment staged file found".into());
        }
        write_new(&selected.staged, &original)?;
        let permissions = fs::metadata(&selected.config)
            .map_err(|e| e.to_string())?
            .permissions();
        if let Err(e) = fs::set_permissions(&selected.staged, permissions)
            .and_then(|()| fs::rename(&selected.staged, &selected.config))
        {
            let _ = fs::remove_file(&selected.staged);
            return Err(format!(
                "cannot restore original Bottles configuration: {e}"
            ));
        }
    }
    fs::remove_file(&selected.marker).map_err(|e| e.to_string())?;
    fs::remove_file(&selected.backup).map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn run_bottle_env(args: Vec<String>) -> Result<(), String> {
    let options = Options::parse(args)?;
    let list = bottles_json_with_command("bottles-cli", &["list", "bottles"])?;
    let names = parse_bottle_names(&list).map_err(|e| e.to_string())?;
    let inventory: Value = serde_json::from_str(&list).map_err(|e| e.to_string())?;
    let root = bottles_json_with_command("bottles-cli", &["info", "bottles-path"])?;
    let mut selections = Vec::new();
    for name in &options.bottles {
        if !names.contains(name) {
            return Err(format!(
                "selected bottle not present in native Bottles: {name}"
            ));
        }
        selections.push(prepare(name, &options.allow, &inventory, &root)?);
    }
    // Global preflight: no bottle is changed if a different selection is invalid.
    for selected in &selections {
        let current = status(selected)?;
        if options.action == "apply" && !selected.missing.is_empty() && current != "unmanaged" {
            return Err(format!(
                "{}: prior managed/environment edit ({current}); restore first",
                selected.name
            ));
        }
        if options.action == "restore" && current == "unmanaged" {
            return Err(format!("{}: nothing owned to restore", selected.name));
        }
        if options.action == "restore" && current == "modified_since_intimatr" {
            return Err(format!(
                "{}: settings changed since apply; refusing restore",
                selected.name
            ));
        }
        if options.action == "apply" && fs::symlink_metadata(&selected.staged).is_ok() {
            return Err(format!("{}: stale staged file found", selected.name));
        }
    }
    for selected in &selections {
        let before = status(selected)?;
        match options.action.as_str() {
            "plan" | "status" => {
                println!(
                    "{}",
                    serde_json::json!({
                        "bottle":selected.name, "operation":options.action,
                        "ownership":before, "allowlist_entries_to_add":selected.missing,
                        "files_modified":false
                    })
                );
            }
            "apply" => {
                apply(selected)?;
                println!(
                    "{}",
                    serde_json::json!({
                        "bottle":selected.name, "operation":"apply",
                        "added":selected.missing, "ownership":status(selected)?,
                        "only_bottle_environment_modified":!selected.missing.is_empty()
                    })
                );
            }
            "restore" => {
                restore(selected)?;
                println!(
                    "{}",
                    serde_json::json!({
                        "bottle":selected.name, "operation":"restore",
                        "ownership":"unmanaged", "original_bottle_yml_restored":true
                    })
                );
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Options, block_list, render};
    const CONFIG: &str = "Name: Test\nLimit_System_Environment: true\nInherited_Environment_Variables:\n- HOME\n- PATH\nEnvironment_Variables: {}\n";
    #[test]
    fn preserves_unrelated_yaml_bytes() {
        let (updated, missing) = render(
            CONFIG,
            &["HOME".into(), "MANGOHUD".into()],
            &["HOME".into(), "PATH".into()],
        )
        .unwrap();
        assert_eq!(missing, vec!["MANGOHUD".to_owned()]);
        assert_eq!(updated, CONFIG.replace("- PATH\n", "- PATH\n- MANGOHUD\n"));
        assert_eq!(
            block_list(&updated).unwrap().1,
            vec!["HOME".to_owned(), "PATH".to_owned(), "MANGOHUD".to_owned()]
        );
    }
    #[test]
    fn refuses_inventory_drift_and_nested_yaml() {
        assert!(render(CONFIG, &["MANGOHUD".into()], &["WRONG".into()]).is_err());
        assert!(block_list("Inherited_Environment_Variables:\n  - HOME\nOther: true\n").is_err());
    }
    #[test]
    fn selection_is_explicit_and_reversible() {
        let parsed = Options::parse(vec![
            "apply".into(),
            "--bottle".into(),
            "Eastward".into(),
            "--bottle".into(),
            "Other".into(),
            "--preset".into(),
            "intimatr-hud".into(),
            "--bottles-closed".into(),
        ])
        .unwrap();
        assert_eq!(parsed.bottles.len(), 2);
        assert!(parsed.allow.contains(&"LD_PRELOAD".to_owned()));
        assert!(
            Options::parse(vec![
                "apply".into(),
                "--bottle".into(),
                "Game".into(),
                "--allow".into(),
                "MANGOHUD".into()
            ])
            .is_err()
        );
        assert!(
            Options::parse(vec![
                "apply".into(),
                "--bottle".into(),
                "..".into(),
                "--allow".into(),
                "MANGOHUD".into(),
                "--bottles-closed".into()
            ])
            .is_err()
        );
    }
}
