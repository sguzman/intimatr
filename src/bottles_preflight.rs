//! Explicit, read-only candidate review before any future Wine game attach.
//!
//! A PID is not a durable identity and a matching image/architecture is not
//! permission to inject a DLL. This module never opens or mutates a process.

use serde::Serialize;

use crate::{bottles_live_matches::ReadOnlyProcessMatchPlan, speed_policy::game_path_key};

#[derive(Debug, Clone, Serialize)]
pub struct CandidatePreflight {
    pub operation: &'static str,
    pub selected_bottle: String,
    pub selected_windows_pid: u32,
    pub selected_executable_path: String,
    pub observed_executable_path: Option<String>,
    pub matched_registered_program: Option<String>,
    pub registration_status: Option<&'static str>,
    pub x64_dll_compatible: Option<bool>,
    /// True only when a single registered program matches this exact PID
    /// and drive-qualified path at the time of this read-only snapshot.
    pub registered_identity_verified_at_snapshot: bool,
    pub status: &'static str,
    pub snapshot_only: bool,
    pub safe_to_attach: bool,
    pub attachment_attempted: bool,
}

/// Validate a requested candidate before launching Wine, even for a
/// supposedly read-only probe (Wine itself may update its prefix metadata).
pub fn validate_explicit_candidate_selection(
    selected_pid: u32,
    selected_executable_path: &str,
) -> Result<(), String> {
    if selected_pid == 0 {
        return Err("selected Windows PID must be positive".into());
    }
    game_path_key(selected_executable_path)
        .map(|_| ())
        .map_err(|error| format!("invalid selected executable path: {error}"))
}

/// Both selectors must be explicit. No basename or PID-only matching is
/// accepted, and even a fully matching x64 snapshot stays review-only.
pub fn assess_explicit_candidate(
    plan: &ReadOnlyProcessMatchPlan,
    selected_pid: u32,
    selected_executable_path: &str,
) -> Result<CandidatePreflight, String> {
    if selected_pid == 0 {
        return Err("selected Windows PID must be positive".into());
    }
    let expected_path = game_path_key(selected_executable_path)
        .map_err(|error| format!("invalid selected executable path: {error}"))?;
    let entry = plan
        .matches
        .iter()
        .find(|entry| entry.windows_pid == selected_pid);
    let observed_path = entry.and_then(|entry| entry.verified_executable_path.clone());
    let path_matches = observed_path
        .as_deref()
        .and_then(|path| game_path_key(path).ok())
        .is_some_and(|path| path == expected_path);
    let registered = entry.is_some_and(|entry| {
        path_matches
            && entry.status == "exact_registered_path"
            && entry.registered_program_name.is_some()
    });
    let architecture = entry.and_then(|entry| entry.x64_dll_compatible);
    let status = match entry {
        None => "selected_pid_not_in_snapshot",
        Some(_) if !path_matches => "selected_executable_does_not_match_pid",
        Some(entry) if entry.status != "exact_registered_path" => entry.status,
        Some(_) if architecture == Some(false) => "unsupported_x64_dll_architecture",
        Some(_) if architecture.is_none() => "architecture_unverified",
        Some(_) => "registered_x64_candidate_for_review_only",
    };
    Ok(CandidatePreflight {
        operation: "read_only_explicit_game_candidate_preflight",
        selected_bottle: plan.bottle.clone(),
        selected_windows_pid: selected_pid,
        selected_executable_path: selected_executable_path.to_owned(),
        observed_executable_path: observed_path,
        matched_registered_program: if registered {
            entry.and_then(|entry| entry.registered_program_name.clone())
        } else {
            None
        },
        registration_status: entry.map(|entry| entry.status),
        x64_dll_compatible: architecture,
        registered_identity_verified_at_snapshot: registered,
        status,
        snapshot_only: true,
        safe_to_attach: false,
        attachment_attempted: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bottles::{BottleInventory, RegisteredProgram},
        bottles_live_matches::{
            WindowsProcessRecord, WindowsProcessSnapshot, match_registered_programs,
        },
    };

    fn plan(
        registered: &[(&str, &str)],
        path: Option<&str>,
        file_arch: Option<&str>,
        process_arch: Option<&str>,
    ) -> ReadOnlyProcessMatchPlan {
        let inventory = BottleInventory {
            bottle: "Games".into(),
            programs: registered
                .iter()
                .map(|(name, path)| RegisteredProgram {
                    name: (*name).into(),
                    executable: "Game.exe".into(),
                    path: (*path).into(),
                    arguments: String::new(),
                })
                .collect(),
        };
        let snapshot = WindowsProcessSnapshot {
            operation: "read_only_wine_windows_processes".into(),
            attachment_attempted: false,
            processes: vec![WindowsProcessRecord {
                windows_pid: 42,
                reported_executable: "Game.exe".into(),
                full_image_path: path.map(str::to_owned),
                image_query_verified: path.is_some(),
                pe_image_architecture: file_arch.map(str::to_owned),
                pe_image_verified: file_arch.is_some(),
                process_architecture: process_arch.map(str::to_owned),
                process_architecture_verified: process_arch.is_some(),
                process_architecture_source: process_arch.map(|_| "is_wow64_process2".into()),
                safe_to_attach: false,
            }],
        };
        match_registered_programs(&inventory, &snapshot)
    }

    #[test]
    fn exact_registered_x64_pid_stays_review_only() {
        let plan = plan(
            &[
                ("First", r"C:\Games\First\Game.exe"),
                ("Second", r"C:\Games\Second\Game.exe"),
            ],
            Some(r"C:\Games\Second\Game.exe"),
            Some("x86_64"),
            Some("x86_64"),
        );
        let audit = assess_explicit_candidate(&plan, 42, "c:/games/second/GAME.exe").unwrap();
        assert_eq!(audit.status, "registered_x64_candidate_for_review_only");
        assert_eq!(audit.matched_registered_program.as_deref(), Some("Second"));
        assert_eq!(audit.x64_dll_compatible, Some(true));
        assert!(audit.registered_identity_verified_at_snapshot);
        assert!(audit.snapshot_only);
        assert!(!audit.safe_to_attach);
        assert!(!audit.attachment_attempted);
    }

    #[test]
    fn missing_pid_wrong_executable_and_invalid_path_fail_closed() {
        let plan = plan(
            &[("Game", r"C:\Games\Game.exe")],
            Some(r"C:\Games\Game.exe"),
            Some("x86_64"),
            Some("x86_64"),
        );
        let missing = assess_explicit_candidate(&plan, 43, r"C:\Games\Game.exe").unwrap();
        assert_eq!(missing.status, "selected_pid_not_in_snapshot");
        let wrong = assess_explicit_candidate(&plan, 42, r"C:\Other\Game.exe").unwrap();
        assert_eq!(wrong.status, "selected_executable_does_not_match_pid");
        assert!(!wrong.registered_identity_verified_at_snapshot);
        assert!(assess_explicit_candidate(&plan, 0, r"C:\Games\Game.exe").is_err());
        assert!(assess_explicit_candidate(&plan, 42, r"C:Games\Game.exe").is_err());
        assert!(assess_explicit_candidate(&plan, 42, r"C:\Games\..\Game.exe").is_err());
    }

    #[test]
    fn missing_architecture_and_x86_do_not_claim_supported_activation() {
        let registry = [("Game", r"C:\Games\Game.exe")];
        let unknown = plan(&registry, Some(r"C:\Games\Game.exe"), None, None);
        let audit = assess_explicit_candidate(&unknown, 42, r"C:\Games\Game.exe").unwrap();
        assert_eq!(audit.status, "architecture_unverified");
        assert_eq!(audit.x64_dll_compatible, None);
        assert!(!audit.safe_to_attach);

        let x86 = plan(
            &registry,
            Some(r"C:\Games\Game.exe"),
            Some("x86"),
            Some("x86"),
        );
        let audit = assess_explicit_candidate(&x86, 42, r"C:\Games\Game.exe").unwrap();
        assert_eq!(audit.status, "unsupported_x64_dll_architecture");
        assert!(!audit.safe_to_attach);
    }

    #[test]
    fn duplicate_registration_and_unregistered_path_are_not_identity_proof() {
        let dupe = plan(
            &[("A", r"C:\Games\Game.exe"), ("B", "c:/games/GAME.exe")],
            Some(r"C:\Games\Game.exe"),
            Some("x86_64"),
            Some("x86_64"),
        );
        let audit = assess_explicit_candidate(&dupe, 42, r"C:\Games\Game.exe").unwrap();
        assert_eq!(audit.status, "ambiguous_duplicate_program_registration");
        assert!(!audit.registered_identity_verified_at_snapshot);

        let unregistered = plan(
            &[("Other", r"C:\Other\Game.exe")],
            Some(r"C:\Games\Game.exe"),
            Some("x86_64"),
            Some("x86_64"),
        );
        let audit = assess_explicit_candidate(&unregistered, 42, r"C:\Games\Game.exe").unwrap();
        assert_eq!(audit.status, "not_registered_in_selected_bottle");
        assert!(!audit.safe_to_attach);
    }
}
