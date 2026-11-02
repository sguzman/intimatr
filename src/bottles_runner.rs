//! Read-only, conservative Bottles runner identity audit.
//!
//! The upstream Bottles manager uses Paths.runners/<Runner> for bundled
//! runners, sys-* for host Wine, and external paths for custom runners.
//! The directory reported by "info bottles-path" does NOT reveal the
//! installed runner root when Bottles uses a custom bottles location.
//! Therefore this audit NEVER infers a runner from a sibling directory.
//! Only an explicitly recorded RunnerPath is treated as candidate evidence.

use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerAudit {
    pub configured_runner_name: Option<String>,
    pub bottle_architecture: Option<String>,
    pub configured_environment: Option<String>,
    pub configured_runner_path: Option<String>,
    pub runner_path_verified: bool,
    pub reason: &'static str,
    pub safe_to_attach: bool,
}

fn field(map: &serde_json::Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match map.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("Bottles {name} must be a string")),
    }
}

/// Only read the selected bottle's configuration. JSON snapshots containing
/// a name-to-config map (Bottles' native CLI representation) are accepted.
pub fn audit_runner_metadata(input: &str, bottle: &str) -> Result<RunnerAudit, String> {
    let root: Value = serde_json::from_str(input)
        .map_err(|error| format!("invalid Bottles inventory: {error}"))?;
    let root = root
        .as_object()
        .ok_or("expected name-keyed Bottles inventory")?;
    let root = root
        .get("bottles")
        .or_else(|| root.get("Bottles"))
        .and_then(Value::as_object)
        .unwrap_or(root);
    let bottle_config = root
        .get(bottle)
        .ok_or("selected bottle is not in runtime inventory")?
        .as_object()
        .ok_or("selected bottle metadata is not an object")?;
    let name = field(bottle_config, "Runner")?;
    let architecture = field(bottle_config, "Arch")?;
    let environment = field(bottle_config, "Environment")?;
    let path = field(bottle_config, "RunnerPath")?;

    // ManagerUtils.get_runner_path: bundled/managed runners are selected
    // from global runner inventory, not the bottle directory. Without a
    // separate runner-directory API, no plausible sibling path is trusted.
    let reason = match (name.as_deref(), environment.as_deref(), path.as_deref()) {
        (None, _, _) => "missing_runner_metadata",
        (Some(n), _, _) if n.starts_with("sys-") => "system_runner_not_proven",
        (Some(_), Some("Steam"), Some(_)) => "steam_runner_path_recorded_not_verified",
        (Some(_), Some("Steam"), None) => "steam_runner_path_missing",
        (Some(_), _, _) => "managed_or_external_runner_location_unknown",
    };

    Ok(RunnerAudit {
        configured_runner_name: name,
        bottle_architecture: architecture,
        configured_environment: environment,
        configured_runner_path: path,
        runner_path_verified: false,
        reason,
        safe_to_attach: false,
    })
}

/// For a specific explicit executable, compare a *recorded* external path.
/// Wine runners may also be selected using a global external-runner mapping,
/// which the per-bottle CLI output does not expose: never infer that map.
pub fn audit_recorded_runner(
    audit: &mut RunnerAudit,
    requested_runner: &Path,
) -> Result<(), String> {
    if audit.configured_environment.as_deref() != Some("Steam") {
        return Ok(());
    }
    let Some(ref root) = audit.configured_runner_path else {
        return Ok(());
    };
    let root = Path::new(root);
    if !root.is_absolute() {
        return Err("recorded Steam runner path is not absolute".into());
    }
    // Steam Proton can be more complex than root/bin/wine, so evidence is
    // limited to exact, existing path equality. This is not a permission
    // to attach even if it succeeds.
    let expected = root.join("bin/wine");
    let expected = match fs::canonicalize(&expected) {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };
    let supplied = match fs::canonicalize(requested_runner) {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };
    if expected != supplied {
        return Err("supplied Wine runner differs from the recorded Steam runner".into());
    }
    let metadata = fs::metadata(&expected)
        .map_err(|error| format!("cannot inspect configured Wine binary: {error}"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("configured Wine runner is not an executable file".into());
    }
    audit.runner_path_verified = true;
    audit.reason = "recorded_steam_runner_binary_matched";
    Ok(())
}

/// Runner identity might be informative while target executable/bitness and
/// Wine process ownership are not proven. Never make it attach authority.
pub fn report_expected_runner(audit: &RunnerAudit) -> Option<PathBuf> {
    if audit.runner_path_verified {
        audit.configured_runner_path.as_ref().map(PathBuf::from)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_runner_name_and_arch_in_native_bottles_map() {
        let sample =
            r#"{"Games":{"Runner":"soda-9.0","Arch":"win64","Environment":"Gaming"},"Empty":{}}"#;
        let audit = audit_runner_metadata(sample, "Games").unwrap();
        assert_eq!(audit.configured_runner_name.as_deref(), Some("soda-9.0"));
        assert_eq!(audit.bottle_architecture.as_deref(), Some("win64"));
        assert!(!audit.runner_path_verified);
        assert!(!audit.safe_to_attach);
        assert_eq!(audit.reason, "managed_or_external_runner_location_unknown");
    }

    #[test]
    fn system_runner_and_steam_runner_require_distinct_evidence() {
        let sys = r#"{"B":{"Runner":"sys-wine","Arch":"win64"}}"#;
        assert_eq!(
            audit_runner_metadata(sys, "B").unwrap().reason,
            "system_runner_not_proven"
        );
        let proton = r#"{"B":{"Runner":"GE-Proton","Environment":"Steam","RunnerPath":"/tmp/GE"}}"#;
        assert_eq!(
            audit_runner_metadata(proton, "B").unwrap().reason,
            "steam_runner_path_recorded_not_verified"
        );
    }

    #[test]
    fn no_runner_path_is_invented_or_implicitly_verified() {
        for item in [
            r#"{"B":{}}"#,
            r#"{"bottles":{"B":{"Runner":"external-custom"}}}"#,
        ] {
            let mut audit = audit_runner_metadata(item, "B").unwrap();
            audit_recorded_runner(&mut audit, Path::new("/usr/bin/wine")).unwrap();
            assert!(!audit.runner_path_verified);
            assert!(report_expected_runner(&audit).is_none());
        }
    }

    #[test]
    fn rejects_untrusted_nonstring_fields_and_missing_bottle() {
        for input in [
            r#"{"B":{"Runner":{}}}"#,
            r#"{"B":{"Arch":3}}"#,
            r#"{"B":{"Environment":true}}"#,
            r#"{"B":{"RunnerPath":[]}}"#,
        ] {
            assert!(audit_runner_metadata(input, "B").is_err());
        }
        assert!(audit_runner_metadata(r#"{"A":{}}"#, "B").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn recorded_steam_runner_must_match_exact_executable() {
        use std::{
            os::unix::fs::symlink,
            sync::atomic::{AtomicUsize, Ordering},
        };
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "intimatr-runner-audit-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/wine"), "synthetic executable").unwrap();
        let mut permissions = fs::metadata(root.join("bin/wine")).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(root.join("bin/wine"), permissions).unwrap();
        let input = format!(
            r#"{{"B":{{"Runner":"GE-Proton","Environment":"Steam","RunnerPath":"{}"}}}}"#,
            root.display()
        );
        let mut audit = audit_runner_metadata(&input, "B").unwrap();
        let alias = root.join("wine-alias");
        symlink(root.join("bin/wine"), &alias).unwrap();
        audit_recorded_runner(&mut audit, &alias).unwrap();
        assert!(audit.runner_path_verified);
        assert_eq!(audit.reason, "recorded_steam_runner_binary_matched");
        let other = root.join("other-wine");
        fs::write(&other, "").unwrap();
        assert!(audit_recorded_runner(&mut audit, &other).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
