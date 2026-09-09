use super::matrix::{ConformanceBoundary, ConformanceCase};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConformanceProof {
    pub case_id: u8,
    pub test: &'static str,
    pub source: &'static str,
}

pub const CONFORMANCE_PROOFS: [ConformanceProof; 40] = [
    proof(
        1,
        "production_bus_and_verified_session_read_from_real_pty",
        "tests/rtu_stack.rs",
    ),
    proof(
        2,
        "mismatch_partial_and_ambiguous_never_create_a_session",
        "tests/rtu_stack.rs",
    ),
    proof(
        3,
        "hangup_reconnects_same_identity_and_rejects_changed_fingerprint",
        "tests/rtu_stack.rs",
    ),
    proof(
        4,
        "case_4_audit_degraded_survives_same_identity_reconnect",
        "tests/conformance_identity_pty.rs",
    ),
    proof(
        5,
        "hangup_reconnects_same_identity_and_rejects_changed_fingerprint",
        "tests/rtu_stack.rs",
    ),
    proof(
        6,
        "case_6_scalar_fault_transitions_and_unknown_flow_through_real_rtu",
        "tests/conformance_faults_pty.rs",
    ),
    proof(
        7,
        "case_7_bitset_simultaneous_known_and_unknown_changes_are_atomic_over_rtu",
        "tests/conformance_faults_pty.rs",
    ),
    proof(
        8,
        "case_8_freeze_frame_complete_partial_and_unavailable_use_real_bus_failures",
        "tests/conformance_faults_pty.rs",
    ),
    proof(
        9,
        "case_9_fault_telemetry_critical_does_not_starve_other_classes",
        "tests/conformance_pressure_pty.rs",
    ),
    proof(
        10,
        "case_10_normal_csv_capture_finishes_with_final_sidecar",
        "tests/conformance_csv_pty.rs",
    ),
    proof(
        11,
        "case_11_slow_csv_consumer_produces_exact_gap_without_blocking_rtu",
        "tests/conformance_csv_pty.rs",
    ),
    proof(
        12,
        "case_12_crash_leaves_running_sidecar_and_runtime_checkpoint",
        "tests/conformance_csv.rs",
    ),
    proof(
        13,
        "case_13_clean_stop_removes_runtime_checkpoint",
        "tests/conformance_csv.rs",
    ),
    proof(
        14,
        "case_14_prepare_confirm_success_is_one_verified_pty_write",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        15,
        "case_15_changed_guard_between_prepare_and_confirm_emits_zero_writes",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        16,
        "case_16_expired_plan_is_consumed_and_never_writes",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        17,
        "case_17_device_exception_is_one_rejected_write",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        18,
        "case_18_applied_write_with_dropped_response_is_outcome_unknown_once_and_disarms",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        19,
        "case_19_delayed_apply_uses_bounded_readback_without_rewrite",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        20,
        "case_20_ignored_write_becomes_readback_mismatch_without_retry",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        21,
        "case_21_preview_raw_cannot_override_active_profile_encoding",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        22,
        "case_22_decision_record_contains_no_prepared_token",
        "tests/conformance_audit.rs",
    ),
    proof(
        23,
        "case_23_audit_prepare_failure_is_sticky_degraded_with_zero_write",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        24,
        "case_24_audit_finalize_failure_degrades_and_blocks_next_write",
        "tests/conformance_write_pty.rs",
    ),
    proof(
        25,
        "case_25_prepared_token_is_single_use_and_context_bound",
        "tests/conformance_audit.rs",
    ),
    proof(
        26,
        "case_26_exact_embedded_manifest_and_qualification_is_packaged",
        "tests/conformance_domain.rs",
    ),
    proof(
        27,
        "case_27_hash_drift_never_receives_packaged_trust",
        "tests/conformance_domain.rs",
    ),
    proof(
        28,
        "case_28_local_profile_requires_exact_hash_approval_before_any_physical_write",
        "tests/conformance_local_trust_pty.rs",
    ),
    proof(
        29,
        "plan_is_profile_ordered_hashed_and_binds_exact_old_and_target",
        "../lantern-app/src/restore.rs",
    ),
    proof(
        30,
        "restore_orders_durable_audit_single_write_verification_and_finalization",
        "../lantern-app/src/write_coordinator_restore_tests.rs",
    ),
    proof(
        31,
        "case_31_non_normal_restore_policies_and_access_classes_are_not_eligible",
        "tests/conformance_restore_policy.rs",
    ),
    proof(
        32,
        "case_32_all_normal_restore_steps_succeed_over_real_rtu",
        "tests/conformance_restore_pty.rs",
    ),
    proof(
        33,
        "case_33_first_middle_and_last_failure_invalidate_permit_and_disarm",
        "tests/conformance_restore_pty.rs",
    ),
    proof(
        34,
        "case_34_outcome_unknown_disconnect_and_audit_degraded_are_terminal",
        "tests/conformance_restore_pty.rs",
    ),
    proof(
        35,
        "case_35_restore_failure_never_retries_rolls_back_or_auto_resumes",
        "tests/conformance_restore_pty.rs",
    ),
    proof(
        36,
        "case_36_above_budget_degrades_deterministically_and_executes_real_rtu",
        "tests/conformance_pressure_pty.rs",
    ),
    proof(
        37,
        "case_37_full_safety_queue_reports_queue_full_explicitly",
        "tests/conformance_pressure_pty.rs",
    ),
    proof(
        38,
        "case_38_safety_burst_yields_to_interactive_before_deadline",
        "tests/conformance_pressure_pty.rs",
    ),
    proof(
        39,
        "case_39_late_timer_advances_without_catch_up_burst",
        "tests/conformance_pressure_pty.rs",
    ),
    proof(
        40,
        "case_40_slow_nonblocking_sink_does_not_inflate_rtu_latency",
        "tests/conformance_pressure_pty.rs",
    ),
];

const fn proof(case_id: u8, test: &'static str, source: &'static str) -> ConformanceProof {
    ConformanceProof {
        case_id,
        test,
        source,
    }
}

#[must_use]
pub fn conformance_proof(case_id: u8) -> Option<&'static ConformanceProof> {
    case_id
        .checked_sub(1)
        .and_then(|index| CONFORMANCE_PROOFS.get(index as usize))
        .filter(|proof| proof.case_id == case_id)
}

#[must_use]
pub fn proof_matches_boundary(case: ConformanceCase, proof: ConformanceProof) -> bool {
    match case.boundary {
        ConformanceBoundary::RtuPty | ConformanceBoundary::RtuPtyWithInjectedPort => {
            proof.source.contains("rtu_stack") || proof.source.contains("_pty.rs")
        }
        ConformanceBoundary::Domain => !proof.source.contains("conformance_csv.rs"),
        ConformanceBoundary::Filesystem => proof.source.contains("conformance_csv.rs"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::matrix::conformance_case;
    use super::*;

    #[test]
    fn every_matrix_case_has_one_boundary_compatible_executable_proof() {
        assert_eq!(CONFORMANCE_PROOFS.len(), 40);
        let mut tests = BTreeSet::new();
        for expected_id in 1..=40_u8 {
            let case = conformance_case(expected_id).expect("matrix case");
            let proof = conformance_proof(expected_id).expect("proof");
            assert_eq!(proof.case_id, expected_id);
            assert!(!proof.test.is_empty());
            assert!(!proof.source.is_empty());
            assert!(proof_matches_boundary(case, *proof), "case {expected_id}");
            tests.insert((proof.source, proof.test));
        }
        assert!(
            tests.len() >= 38,
            "shared reconnect test may prove cases 3 and 5 only"
        );
    }
}
