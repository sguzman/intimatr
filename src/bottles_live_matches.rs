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
            if process.image_query_verified != process.full_image_path.is_some() {
                return Err("Windows process snapshot has inconsistent image verification".into());
            }
            if process.pe_image_verified != process.pe_image_architecture.is_some() {
                return Err(
                    "Windows process snapshot has inconsistent PE image verification".into(),
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
        let status = if !process.image_query_verified {
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
