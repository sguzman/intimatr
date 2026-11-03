//! Read-only static PE compatibility report for an offline executable.
//! A successful file scan is NOT proof of Wine resolution, ownership, actual
//! engine timing usage or permission to attach to any process.

use std::{
    fs,
    path::{Path, PathBuf},
};

use intimatr::pe_timer_imports::{TimerImportCoverage, inspect_pe_timer_imports};

fn parse_executable_path(args: &[String]) -> Result<PathBuf, String> {
    let [flag, raw] = args else {
        return Err("pe-compat requires exactly --exe ABSOLUTE_LINUX_PE_FILE".into());
    };
    if flag != "--exe" || raw.is_empty() || raw.starts_with("--") {
        return Err("pe-compat requires exactly --exe ABSOLUTE_LINUX_PE_FILE".into());
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err("pe-compat requires an absolute Linux file path".into());
    }
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve selected PE file: {error}"))?;
    let metadata = fs::metadata(&resolved)
        .map_err(|error| format!("cannot inspect selected PE file: {error}"))?;
    if !metadata.is_file() {
        return Err("pe-compat requires a regular executable file".into());
    }
    Ok(resolved)
}

/// Categorize static *names* only. None is indeterminate, never a false
/// missing-import verdict. This function deliberately issues no attach claim.
/// Shared by single-PE and selected-bottle read-only inventory.
pub(super) fn classify_imports(coverage: Result<TimerImportCoverage, String>) -> serde_json::Value {
    let (state, imports, missing, error) = match coverage {
        Ok(observed) if observed.is_complete() => (
            "static_four_timer_names_present",
            Some(observed),
            Vec::new(),
            None,
        ),
        Ok(observed) => (
            "static_timer_imports_missing",
            Some(observed),
            observed.missing_names(),
            None,
        ),
        Err(error) => (
            "static_import_inventory_unknown",
            None,
            Vec::new(),
            Some(error),
        ),
    };
    serde_json::json!({
        "static_import_status": state,
        "static_timer_import_coverage": imports,
        "static_timer_imports_missing": if imports.is_some() {
            Some(missing)
        } else {
            None
        },
        "inspection_error": error,
        "requires_live_wine_import_verification": true,
        "gameplay_speed_verified": false,
        "process_attachment_attempted": false,
        "timer_hook_installation_attempted": false,
    })
}

fn report(path: &Path, coverage: Result<TimerImportCoverage, String>) -> serde_json::Value {
    let mut result = classify_imports(coverage);
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "operation".into(),
            serde_json::json!("read_only_linux_pe_timer_compatibility"),
        );
        object.insert("selected_file".into(), serde_json::json!(path));
    }
    result
}

pub(super) fn run_pe_compat(args: Vec<String>) -> Result<(), String> {
    let path = parse_executable_path(&args)?;
    let output = report(&path, inspect_pe_timer_imports(&path));
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("cannot serialize read-only PE report: {error}"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_one_absolute_file_selector() {
        for args in [
            vec![],
            vec!["--exe".into()],
            vec!["--exe".into(), "Game.exe".into()],
            vec!["--wrong".into(), "/tmp/Game.exe".into()],
            vec!["--exe".into(), "/tmp/Game.exe".into(), "--extra".into()],
        ] {
            assert!(parse_executable_path(&args).is_err());
        }
    }

    #[test]
    fn distinguishes_missing_full_and_unknown_without_gameplay_claim() {
        let path = Path::new("/tmp/disposable-example.exe");
        let empty = report(path, Ok(TimerImportCoverage::default()));
        assert_eq!(
            empty["static_import_status"],
            "static_timer_imports_missing"
        );
        assert_eq!(
            empty["static_timer_imports_missing"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        let mut full = TimerImportCoverage {
            query_performance_counter: true,
            get_tick_count64: true,
            get_tick_count: true,
            time_get_time: true,
            all_four_static_names: true,
        };
        let complete = report(path, Ok(full));
        assert_eq!(
            complete["static_import_status"],
            "static_four_timer_names_present"
        );
        assert_eq!(
            complete["static_timer_imports_missing"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        full.time_get_time = false;
        full.all_four_static_names = false;
        let partial = report(path, Ok(full));
        assert_eq!(
            partial["static_timer_imports_missing"],
            serde_json::json!(["timeGetTime"])
        );
        let unknown = report(path, Err("invalid PE".into()));
        assert_eq!(
            unknown["static_import_status"],
            "static_import_inventory_unknown"
        );
        assert!(unknown["static_timer_imports_missing"].is_null());
        assert_eq!(unknown["inspection_error"], "invalid PE");
        for item in [empty, complete, partial, unknown] {
            assert_eq!(item["process_attachment_attempted"], false);
            assert_eq!(item["timer_hook_installation_attempted"], false);
            assert_eq!(item["gameplay_speed_verified"], false);
        }
    }
}
