//! Portable, read-only reconciliation of a Wine Windows PID snapshot with
//! registered Bottles programs in ONE explicitly selected bottle.
//!
//! A matching path is evidence for review, not attach authorization.

use serde::{Deserialize, Serialize};

use crate::{bottles::BottleInventory, speed_policy::game_path_key};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsProcessRecord {
    pub windows_pid: u32,
    pub reported_executable: String,
    pub full_image_path: Option<String>,
    pub image_query_verified: bool,
    #[serde(default)]
    pub pe_image_architecture: Option<String>,
    #[serde(default)]
    pub pe_image_verified: bool,
    #[serde(default)]
    pub process_architecture: Option<String>,
    #[serde(default)]
    pub process_architecture_verified: bool,
    #[serde(default)]
    pub process_architecture_source: Option<String>,
    /// Decimal FILETIME text avoids JSON floating-point precision loss.
    #[serde(default)]
    pub process_creation_time_100ns: Option<String>,
    pub safe_to_attach: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsProcessSnapshot {
    pub operation: String,
    pub processes: Vec<WindowsProcessRecord>,
    pub attachment_attempted: bool,
}

impl WindowsProcessSnapshot {
    pub fn parse(input: &str) -> Result<Self, String> {
        let result: Self = serde_json::from_str(input)
            .map_err(|error| format!("invalid Windows process snapshot: {error}"))?;
        if result.operation != "read_only_wine_windows_processes" || result.attachment_attempted {
            return Err("expected a read-only Wine process snapshot".into());
        }
        let mut pids = std::collections::HashSet::new();
        for process in &result.processes {
            if process.windows_pid == 0 || !pids.insert(process.windows_pid) {
                return Err("Windows process snapshot has duplicate or zero PID".into());
            }
            if process.safe_to_attach {
                return Err("Windows process snapshot unexpectedly claims attach authority".into());
            }
            if process
                .process_creation_time_100ns
                .as_deref()
                .is_some_and(|value| {
                    value.parse::<u64>().ok().filter(|number| *number > 0)
                        .is_none_or(|number| number.to_string() != value)
                })
            {
                return Err("Windows process snapshot has invalid creation-time identity".into());
            }
            if process.image_query_verified != process.full_image_path.is_some() {
                return Err("Windows process snapshot has inconsistent image verification".into());
            }
            if process.pe_image_verified != process.pe_image_architecture.is_some() {
                return Err(
                    "Windows process snapshot has inconsistent PE image verification".into(),
                );
            }
            if process.process_architecture_verified
                != (process.process_architecture.is_some()
                    && process.process_architecture_source.is_some())
            {
                return Err(
                    "Windows process snapshot has inconsistent live machine evidence".into(),
                );
            }
            if process
                .process_architecture_source
                .as_deref()
                .is_some_and(|source| source != "is_wow64_process2")
            {
                return Err("Windows process snapshot has unrecognized architecture source".into());
            }
            if process
                .process_architecture
                .as_deref()
                .is_some_and(|value| !matches!(value, "x86" | "x86_64" | "aarch64"))
            {
                return Err(
                    "Windows process snapshot has unrecognized live process machine".into(),
                );
            }
            if process
                .pe_image_architecture
                .as_deref()
                .is_some_and(|value| !matches!(value, "x86" | "x86_64" | "aarch64"))
            {
                return Err("Windows process snapshot reports unsupported PE architecture".into());
            }
            if process
                .full_image_path
                .as_ref()
                .is_some_and(|path| path.trim().is_empty())
            {
                return Err("Windows process snapshot contains empty image path".into());
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessProgramMatch {
    pub windows_pid: u32,
    pub verified_executable_path: Option<String>,
    pub pe_image_architecture: Option<String>,
    pub pe_image_verified: bool,
    pub process_architecture: Option<String>,
    pub process_architecture_verified: bool,
    pub process_architecture_source: Option<String>,
    pub process_creation_time_100ns: Option<String>,
    pub architecture_consistent: Option<bool>,
    /// True only when BOTH the live process and file are confirmed x86-64.
    /// False means both were observed but this x86-64 DLL cannot fit.
    /// None means architecture proof is incomplete.
    pub x64_dll_compatible: Option<bool>,
    pub registered_program_name: Option<String>,
    pub status: &'static str,
    pub safe_to_attach: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadOnlyProcessMatchPlan {
    pub operation: &'static str,
    pub bottle: String,
    pub matches: Vec<ProcessProgramMatch>,
    pub attachment_attempted: bool,
}

/// Never infer a Windows program from its process basename. Only an exactly
/// normalized absolute Windows image path can yield an unambiguous match.
pub fn match_registered_programs(
    bottle: &BottleInventory,
    snapshot: &WindowsProcessSnapshot,
) -> ReadOnlyProcessMatchPlan {
    let mut matches = Vec::new();
    for process in &snapshot.processes {
        let mut found = None;
        let architecture_consistent = process
            .process_architecture
            .as_ref()
            .zip(process.pe_image_architecture.as_ref())
            .map(|(observed, file)| observed == file);
        let x64_dll_compatible = process
            .process_architecture
            .as_deref()
            .zip(process.pe_image_architecture.as_deref())
            .map(|(process_machine, file_machine)| {
                process_machine == "x86_64" && file_machine == "x86_64"
            });
        let status = if architecture_consistent == Some(false) {
            "process_file_architecture_conflict"
        } else if x64_dll_compatible == Some(false) {
            "unsupported_x64_dll_architecture"
        } else if !process.image_query_verified {
            "image_path_unknown"
        } else if let Some(path) = process.full_image_path.as_deref() {
            if let Ok(key) = game_path_key(path) {
                let candidates: Vec<_> = bottle
                    .programs
                    .iter()
                    .filter(|program| {
                        game_path_key(&program.path)
                            .is_ok_and(|registered_path| registered_path == key)
                    })
                    .collect();
                match candidates.as_slice() {
                    [program] => {
                        found = Some(program.name.clone());
                        "exact_registered_path"
                    }
                    [] => "not_registered_in_selected_bottle",
                    _ => "ambiguous_duplicate_program_registration",
                }
            } else {
                "unsupported_windows_path"
            }
        } else {
            "image_path_unknown"
        };
        matches.push(ProcessProgramMatch {
            windows_pid: process.windows_pid,
            verified_executable_path: process.full_image_path.clone(),
            pe_image_architecture: process.pe_image_architecture.clone(),
            pe_image_verified: process.pe_image_verified,
            process_architecture: process.process_architecture.clone(),
            process_architecture_verified: process.process_architecture_verified,
            process_architecture_source: process.process_architecture_source.clone(),
            process_creation_time_100ns: process.process_creation_time_100ns.clone(),
            architecture_consistent,
            x64_dll_compatible,
            registered_program_name: found,
            status,
            safe_to_attach: false,
        });
    }
    matches.sort_by_key(|entry| entry.windows_pid);
    ReadOnlyProcessMatchPlan {
        operation: "read_only_wine_registered_program_matching",
        bottle: bottle.bottle.clone(),
        matches,
        attachment_attempted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottles::RegisteredProgram;

    fn bottle(programs: &[(&str, &str)]) -> BottleInventory {
        BottleInventory {
            bottle: "Owned".into(),
            programs: programs
                .iter()
                .map(|(name, path)| RegisteredProgram {
                    name: (*name).into(),
                    executable: "Game.exe".into(),
                    path: (*path).into(),
                    arguments: String::new(),
                })
                .collect(),
        }
    }

    fn snapshot(records: &[(u32, Option<&str>)]) -> WindowsProcessSnapshot {
        WindowsProcessSnapshot {
            operation: "read_only_wine_windows_processes".into(),
            attachment_attempted: false,
            processes: records
                .iter()
                .map(|(pid, path)| WindowsProcessRecord {
                    windows_pid: *pid,
                    reported_executable: "Game.exe".into(),
                    full_image_path: path.map(str::to_owned),
                    image_query_verified: path.is_some(),
                    pe_image_architecture: None,
                    pe_image_verified: false,
                    process_architecture: None,
                    process_architecture_verified: false,
                    process_architecture_source: None,
                    process_creation_time_100ns: None,
                    safe_to_attach: false,
                })
                .collect(),
        }
    }

    #[test]
    fn matches_full_windows_path_without_guessing_duplicate_basenames() {
        let bottle = bottle(&[
            ("First", r"C:\Games\First\Game.exe"),
            ("Second", r"C:\Games\Second\Game.exe"),
        ]);
        let matches = match_registered_programs(
            &bottle,
            &snapshot(&[
                (5, Some("c:/games/second/GAME.exe")),
                (6, Some(r"C:\Games\Unregistered\Game.exe")),
                (7, None),
            ]),
        );
        assert_eq!(
            matches.matches[0].registered_program_name.as_deref(),
            Some("Second")
        );
        assert_eq!(matches.matches[0].status, "exact_registered_path");
        assert_eq!(
            matches.matches[1].status,
            "not_registered_in_selected_bottle"
        );
        assert_eq!(matches.matches[2].status, "image_path_unknown");
        assert!(matches.matches.iter().all(|record| !record.safe_to_attach));
    }

    #[test]
    fn duplicate_registered_path_is_ambiguous_not_selected() {
        let bottle = bottle(&[
            ("First", r"C:\Games\Game.exe"),
            ("Duplicate", "c:/games/GAME.exe"),
        ]);
        let plan =
            match_registered_programs(&bottle, &snapshot(&[(8, Some(r"C:\Games\Game.exe"))]));
        assert_eq!(
            plan.matches[0].status,
            "ambiguous_duplicate_program_registration"
        );
        assert!(plan.matches[0].registered_program_name.is_none());
    }

    #[test]
    fn rejects_forged_or_inconsistent_pid_snapshots() {
        let valid = r#"{"operation":"read_only_wine_windows_processes","attachment_attempted":false,"processes":[{"windows_pid":2,"reported_executable":"Demo.exe","full_image_path":"C:\\Demo.exe","image_query_verified":true,"safe_to_attach":false}]}"#;
        assert_eq!(
            WindowsProcessSnapshot::parse(valid)
                .unwrap()
                .processes
                .len(),
            1
        );
        for invalid in [
            valid.replace(
                "\"attachment_attempted\":false",
                "\"attachment_attempted\":true",
            ),
            valid.replace("\"safe_to_attach\":false", "\"safe_to_attach\":true"),
            valid.replace(
                "\"image_query_verified\":true",
                "\"image_query_verified\":false",
            ),
            valid.replace("\"windows_pid\":2", "\"windows_pid\":0"),
        ] {
            assert!(WindowsProcessSnapshot::parse(&invalid).is_err());
        }
    }

    #[test]
    fn rejects_forged_live_machine_evidence_and_conflicting_file_architecture() {
        let raw = r#"{"operation":"read_only_wine_windows_processes","attachment_attempted":false,"processes":[{"windows_pid":42,"reported_executable":"Game.exe","full_image_path":"C:\\Game.exe","image_query_verified":true,"pe_image_architecture":"x86_64","pe_image_verified":true,"process_architecture":"x86","process_architecture_verified":true,"process_architecture_source":"is_wow64_process2","safe_to_attach":false}]}"#;
        let parsed = WindowsProcessSnapshot::parse(raw).unwrap();
        let matched = match_registered_programs(&bottle(&[("Game", r"C:\Game.exe")]), &parsed);
        assert_eq!(
            matched.matches[0].status,
            "process_file_architecture_conflict"
        );
        assert_eq!(matched.matches[0].architecture_consistent, Some(false));
        assert!(matched.matches[0].registered_program_name.is_none());
        assert!(!matched.matches[0].safe_to_attach);

        for invalid in [
            raw.replace(
                "\"process_architecture_verified\":true",
                "\"process_architecture_verified\":false",
            ),
            raw.replace(
                "\"process_architecture_source\":\"is_wow64_process2\"",
                "\"process_architecture_source\":\"untrusted\"",
            ),
            raw.replace(
                "\"process_architecture\":\"x86\"",
                "\"process_architecture\":\"unknown\"",
            ),
        ] {
            assert!(WindowsProcessSnapshot::parse(&invalid).is_err());
        }
    }

    #[test]
    fn blocks_x86_process_even_when_file_and_registered_executable_agree() {
        let input = r#"{"operation":"read_only_wine_windows_processes","attachment_attempted":false,"processes":[{"windows_pid":55,"reported_executable":"Game.exe","full_image_path":"C:\\Game.exe","image_query_verified":true,"pe_image_architecture":"x86","pe_image_verified":true,"process_architecture":"x86","process_architecture_verified":true,"process_architecture_source":"is_wow64_process2","safe_to_attach":false}]}"#;
        let snapshot = WindowsProcessSnapshot::parse(input).unwrap();
        let result = match_registered_programs(&bottle(&[("Game", r"C:\Game.exe")]), &snapshot);
        assert_eq!(result.matches[0].architecture_consistent, Some(true));
        assert_eq!(result.matches[0].x64_dll_compatible, Some(false));
        assert_eq!(result.matches[0].status, "unsupported_x64_dll_architecture");
        assert!(result.matches[0].registered_program_name.is_none());
        assert!(!result.matches[0].safe_to_attach);
    }

    #[test]
    fn rejects_unverified_unc_and_drive_relative_images() {
        let bottle = bottle(&[("Demo", r"C:\Demo.exe")]);
        let plan = match_registered_programs(
            &bottle,
            &snapshot(&[
                (10, Some(r"\\server\share\Demo.exe")),
                (11, Some("C:Demo.exe")),
            ]),
        );
        assert!(
            plan.matches
                .iter()
                .all(|entry| entry.status == "unsupported_windows_path")
        );
    }
}
