//! Consolidate read-only Bottles prefix, runner and selected Windows process
//! evidence before designing any non-fixture remote loader.
//!
//! A verified snapshot is NOT authority to attach: a PID can be reused, a
//! runner may differ from the bottle configuration, and a game may be
//! protected. This module performs no I/O and never mutates a process.

use serde::Serialize;

use crate::{
    bottles_prefix::BottlePrefixIdentity,
    bottles_preflight::CandidatePreflight,
    bottles_runner::RunnerAudit,
    pe_timer_imports::TimerImportCoverage,
};

#[derive(Debug, Clone, Serialize)]
pub struct AttachmentReadiness {
    pub operation: &'static str,
    pub selected_bottle: String,
    pub selected_windows_pid: u32,
    pub selected_executable_path: String,
    pub candidate_status: &'static str,
    pub snapshot_registered_identity_verified: bool,
    pub snapshot_x64_dll_compatible: Option<bool>,
    /// Static main EXE names, not actual Wine-resolved hook eligibility.
    pub timer_import_coverage: Option<TimerImportCoverage>,
    pub selected_process_creation_time_100ns: Option<String>,
    pub selected_pid_observed_in_prefix_wineserver_snapshot: bool,
    pub selected_image_observed_in_prefix_wineserver_snapshot: bool,
    pub bottle_prefix_verified: bool,
    pub configured_runner_verified: bool,
    pub game_process_prefix_ownership_verified: bool,
    pub blockers: Vec<&'static str>,
    pub status: &'static str,
    pub safe_to_attach: bool,
    pub attachment_attempted: bool,
}

/// Combine distinct pieces of evidence without converting one into another.
/// In particular, verified bottle storage does not prove which live process
/// owns that prefix. Even perfect snapshot evidence cannot enable loading.
pub fn review_attachment_readiness(
    candidate: &CandidatePreflight,
    prefix: &BottlePrefixIdentity,
    runner: &RunnerAudit,
) -> AttachmentReadiness {
    let mut blockers = Vec::new();
    if candidate.selected_bottle != prefix.bottle {
        blockers.push("selected_bottle_prefix_identity_mismatch");
    }
    if !prefix.bottle_prefix_association_verified {
        blockers.push("bottle_prefix_unverified");
    }
    if !runner.runner_path_verified {
        blockers.push("configured_wine_runner_unverified");
    }
    if candidate.process_creation_time_100ns.is_none() {
        blockers.push("selected_process_creation_time_unverified");
    }
    if !candidate.pid_observed_in_selected_wineserver_snapshot {
        blockers.push("selected_pid_not_observed_in_prefix_wineserver_snapshot");
    } else if !candidate.image_observed_in_selected_wineserver_snapshot {
        blockers.push("selected_image_not_observed_in_prefix_wineserver_snapshot");
    }
    if !candidate.registered_identity_verified_at_snapshot
        || candidate.status != "registered_x64_candidate_for_review_only"
    {
        blockers.push("selected_pid_executable_registration_unverified");
    }
    match candidate.x64_dll_compatible {
        Some(true) => {}
        Some(false) => blockers.push("incompatible_x64_dll_architecture"),
        None => blockers.push("live_and_file_architecture_unverified"),
    }
    if !prefix.game_process_identity_verified {
        blockers.push("live_process_prefix_ownership_unverified");
    }
    if !candidate
        .timer_import_coverage
        .is_some_and(|coverage| coverage.all_four_static_names)
    {
        blockers.push("experimental_four_timer_static_imports_unverified");
    }

    // These checks require a separate, opt-in, non-fixture loader design
    // and fresh process identity / game compatibility verification.
    blockers.push("fresh_process_identity_at_apply_not_proven");
    blockers.push("game_protection_status_unverified");
    blockers.push("production_game_loader_field_verification_missing");
    blockers.push("production_hook_lifecycle_not_verified");

    let snapshot_reviewable = candidate.registered_identity_verified_at_snapshot
        && candidate.status == "registered_x64_candidate_for_review_only"
        && candidate.x64_dll_compatible == Some(true);
    AttachmentReadiness {
        operation: "read_only_live_game_attachment_readiness",
        selected_bottle: candidate.selected_bottle.clone(),
        selected_windows_pid: candidate.selected_windows_pid,
        selected_executable_path: candidate.selected_executable_path.clone(),
        candidate_status: candidate.status,
        snapshot_registered_identity_verified: candidate.registered_identity_verified_at_snapshot,
        snapshot_x64_dll_compatible: candidate.x64_dll_compatible,
        timer_import_coverage: candidate.timer_import_coverage,
        selected_process_creation_time_100ns: candidate.process_creation_time_100ns.clone(),
        selected_pid_observed_in_prefix_wineserver_snapshot: candidate
            .pid_observed_in_selected_wineserver_snapshot,
        selected_image_observed_in_prefix_wineserver_snapshot: candidate
            .image_observed_in_selected_wineserver_snapshot,
        bottle_prefix_verified: prefix.bottle_prefix_association_verified,
        configured_runner_verified: runner.runner_path_verified,
        game_process_prefix_ownership_verified: prefix.game_process_identity_verified,
        blockers,
        status: if snapshot_reviewable {
            "snapshot_candidate_for_review_only"
        } else {
            "candidate_identity_or_architecture_unverified"
        },
        safe_to_attach: false,
        attachment_attempted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> (CandidatePreflight, BottlePrefixIdentity, RunnerAudit) {
        let candidate = CandidatePreflight {
            operation: "read_only_explicit_game_candidate_preflight",
            selected_bottle: "Games".into(),
            selected_windows_pid: 42,
            selected_executable_path: r"C:\Games\Test.exe".into(),
            observed_executable_path: Some(r"C:\Games\Test.exe".into()),
            matched_registered_program: Some("Test".into()),
            registration_status: Some("exact_registered_path"),
            x64_dll_compatible: Some(true),
            timer_import_coverage: None,
            process_creation_time_100ns: Some("133700000000000000".into()),
            pid_observed_in_selected_wineserver_snapshot: true,
            image_observed_in_selected_wineserver_snapshot: true,
            registered_identity_verified_at_snapshot: true,
            status: "registered_x64_candidate_for_review_only",
            snapshot_only: true,
            safe_to_attach: false,
            attachment_attempted: false,
        };
        let prefix = BottlePrefixIdentity {
            bottle: "Games".into(),
            canonical_bottles_root: PathBuf::from("/tmp/bottles"),
            canonical_bottle_prefix: PathBuf::from("/tmp/bottles/Games"),
            bottle_prefix_association_verified: true,
            game_process_identity_verified: false,
            runner_verified: false,
            safe_to_attach: false,
        };
        let runner = RunnerAudit {
            configured_runner_name: Some("test".into()),
            bottle_architecture: Some("win64".into()),
            configured_environment: Some("Gaming".into()),
            configured_runner_path: None,
            runner_path_verified: false,
            reason: "managed_or_external_runner_location_unknown",
            safe_to_attach: false,
        };
        (candidate, prefix, runner)
    }

    #[test]
    fn x64_exact_registration_still_never_grants_attachment() {
        let (candidate, prefix, mut runner) = fixtures();
        runner.runner_path_verified = true;
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert_eq!(report.status, "snapshot_candidate_for_review_only");
        assert!(report.snapshot_registered_identity_verified);
        assert!(report.selected_pid_observed_in_prefix_wineserver_snapshot);
        assert!(report.selected_image_observed_in_prefix_wineserver_snapshot);
        assert!(!report.safe_to_attach);
        assert!(!report.attachment_attempted);
        assert!(
            !report
                .blockers
                .contains(&"configured_wine_runner_unverified")
        );
        assert!(
            report
                .blockers
                .contains(&"live_process_prefix_ownership_unverified")
        );
        assert!(
            report
                .blockers
                .contains(&"production_game_loader_field_verification_missing")
        );
    }

    #[test]
    fn wrong_pid_or_path_and_unknown_architecture_reveal_blockers() {
        let (mut candidate, prefix, runner) = fixtures();
        candidate.registered_identity_verified_at_snapshot = false;
        candidate.pid_observed_in_selected_wineserver_snapshot = false;
        candidate.image_observed_in_selected_wineserver_snapshot = false;
        candidate.status = "selected_pid_not_in_snapshot";
        candidate.x64_dll_compatible = None;
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert_eq!(
            report.status,
            "candidate_identity_or_architecture_unverified"
        );
        assert!(!report.selected_pid_observed_in_prefix_wineserver_snapshot);
        assert!(
            report
                .blockers
                .contains(&"selected_pid_not_observed_in_prefix_wineserver_snapshot")
        );
        assert!(
            report
                .blockers
                .contains(&"selected_pid_executable_registration_unverified")
        );
        assert!(
            report
                .blockers
                .contains(&"live_and_file_architecture_unverified")
        );
        assert!(
            report
                .blockers
                .contains(&"configured_wine_runner_unverified")
        );
    }

    #[test]
    fn even_forged_positive_prefix_and_runner_evidence_do_not_enable_attach() {
        let (candidate, mut prefix, mut runner) = fixtures();
        prefix.game_process_identity_verified = true;
        runner.runner_path_verified = true;
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert!(!report.safe_to_attach);
        assert!(
            report
                .blockers
                .contains(&"fresh_process_identity_at_apply_not_proven")
        );
        assert!(
            report
                .blockers
                .contains(&"production_game_loader_field_verification_missing")
        );
    }

    #[test]
    fn correct_pid_but_wrong_executable_never_verifies_image() {
        let (mut candidate, prefix, runner) = fixtures();
        candidate.image_observed_in_selected_wineserver_snapshot = false;
        candidate.registered_identity_verified_at_snapshot = false;
        candidate.status = "selected_executable_does_not_match_pid";
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert!(report.selected_pid_observed_in_prefix_wineserver_snapshot);
        assert!(!report.selected_image_observed_in_prefix_wineserver_snapshot);
        assert!(
            report
                .blockers
                .contains(&"selected_image_not_observed_in_prefix_wineserver_snapshot")
        );
        assert!(!report.safe_to_attach);
    }

    #[test]
    fn complete_static_timer_name_evidence_does_not_claim_hook_or_attachment() {
        let (mut candidate, prefix, runner) = fixtures();
        candidate.timer_import_coverage = Some(TimerImportCoverage {
            query_performance_counter: true,
            get_tick_count64: true,
            get_tick_count: true,
            time_get_time: true,
            all_four_static_names: true,
        });
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert_eq!(
            report.timer_import_coverage.map(|value| value.all_four_static_names),
            Some(true)
        );
        assert!(!report.blockers.contains(&"experimental_four_timer_static_imports_unverified"));
        assert!(!report.safe_to_attach);
        assert!(!report.attachment_attempted);
    }

    #[test]
    fn mismatched_bottle_identity_and_x86_target_fail_closed() {
        let (mut candidate, mut prefix, runner) = fixtures();
        prefix.bottle = "Another".into();
        candidate.x64_dll_compatible = Some(false);
        candidate.status = "unsupported_x64_dll_architecture";
        let report = review_attachment_readiness(&candidate, &prefix, &runner);
        assert!(
            report
                .blockers
                .contains(&"selected_bottle_prefix_identity_mismatch")
        );
        assert!(
            report
                .blockers
                .contains(&"incompatible_x64_dll_architecture")
        );
        assert_eq!(
            report.status,
            "candidate_identity_or_architecture_unverified"
        );
        assert!(!report.safe_to_attach);
    }
}
