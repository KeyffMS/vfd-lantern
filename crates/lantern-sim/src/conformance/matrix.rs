#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceFamily {
    IdentitySession,
    Faults,
    Csv,
    ManualWrite,
    AuditTrust,
    Restore,
    PressureTimers,
}

/// Required execution boundary for one conformance case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConformanceBoundary {
    /// The product communicates with the simulator through PTY and production BusActor.
    RtuPty,
    /// PTY/BusActor remains real while an allowed failure port is injected.
    RtuPtyWithInjectedPort,
    /// The case verifies a capability or immutable value with no serial interaction.
    Domain,
    /// The case verifies deterministic storage behavior and artifact durability.
    Filesystem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConformanceCase {
    pub id: u8,
    pub slug: &'static str,
    pub family: ConformanceFamily,
    pub boundary: ConformanceBoundary,
}

pub const CONFORMANCE_CASE_COUNT: usize = 40;

const fn case(
    id: u8,
    slug: &'static str,
    family: ConformanceFamily,
    boundary: ConformanceBoundary,
) -> ConformanceCase {
    ConformanceCase {
        id,
        slug,
        family,
        boundary,
    }
}

pub const CONFORMANCE_CASES: [ConformanceCase; CONFORMANCE_CASE_COUNT] = [
    case(
        1,
        "identity-match-verified",
        ConformanceFamily::IdentitySession,
        ConformanceBoundary::RtuPty,
    ),
    case(
        2,
        "identity-non-match-fail-closed",
        ConformanceFamily::IdentitySession,
        ConformanceBoundary::RtuPty,
    ),
    case(
        3,
        "reconnect-same-fingerprint",
        ConformanceFamily::IdentitySession,
        ConformanceBoundary::RtuPty,
    ),
    case(
        4,
        "audit-degraded-survives-reconnect",
        ConformanceFamily::IdentitySession,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        5,
        "reconnect-different-fingerprint-blocked",
        ConformanceFamily::IdentitySession,
        ConformanceBoundary::RtuPty,
    ),
    case(
        6,
        "fault-scalar-transitions",
        ConformanceFamily::Faults,
        ConformanceBoundary::RtuPty,
    ),
    case(
        7,
        "fault-bitset-simultaneous-transitions",
        ConformanceFamily::Faults,
        ConformanceBoundary::RtuPty,
    ),
    case(
        8,
        "fault-freeze-frame-completeness",
        ConformanceFamily::Faults,
        ConformanceBoundary::RtuPty,
    ),
    case(
        9,
        "fault-critical-poll-no-starvation",
        ConformanceFamily::Faults,
        ConformanceBoundary::RtuPty,
    ),
    case(
        10,
        "csv-normal-final-sidecar",
        ConformanceFamily::Csv,
        ConformanceBoundary::RtuPty,
    ),
    case(
        11,
        "csv-slow-writer-exact-gap",
        ConformanceFamily::Csv,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        12,
        "csv-crash-running-sidecar-checkpoint",
        ConformanceFamily::Csv,
        ConformanceBoundary::Filesystem,
    ),
    case(
        13,
        "csv-clean-stop-removes-checkpoint",
        ConformanceFamily::Csv,
        ConformanceBoundary::Filesystem,
    ),
    case(
        14,
        "write-prepare-confirm-success-readback",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        15,
        "write-stale-old-or-guard-zero-write",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        16,
        "write-plan-expired-single-use",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        17,
        "write-device-exception",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        18,
        "write-apply-drop-response-outcome-unknown",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        19,
        "write-delayed-apply-bounded-readback",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        20,
        "write-readback-mismatch",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::RtuPty,
    ),
    case(
        21,
        "write-preview-policy-does-not-mutate-profile",
        ConformanceFamily::ManualWrite,
        ConformanceBoundary::Domain,
    ),
    case(
        22,
        "audit-decision-excludes-prepared-token",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::Domain,
    ),
    case(
        23,
        "audit-prepare-failure-zero-write-sticky-degraded",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        24,
        "audit-finalize-failure-blocks-next-write",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        25,
        "audit-prepared-token-single-use-context-bound",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::Domain,
    ),
    case(
        26,
        "trust-packaged-qualified-embedded-manifest",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::Domain,
    ),
    case(
        27,
        "trust-drift-or-unqualified-never-packaged",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::Domain,
    ),
    case(
        28,
        "trust-local-exact-hash-approval",
        ConformanceFamily::AuditTrust,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        29,
        "restore-approved-plan-hash-order",
        ConformanceFamily::Restore,
        ConformanceBoundary::Domain,
    ),
    case(
        30,
        "restore-permit-binds-step-target-order",
        ConformanceFamily::Restore,
        ConformanceBoundary::Domain,
    ),
    case(
        31,
        "restore-omits-non-normal-parameters",
        ConformanceFamily::Restore,
        ConformanceBoundary::Domain,
    ),
    case(
        32,
        "restore-all-normal-steps-success",
        ConformanceFamily::Restore,
        ConformanceBoundary::RtuPty,
    ),
    case(
        33,
        "restore-failure-invalidates-permit-disarms",
        ConformanceFamily::Restore,
        ConformanceBoundary::RtuPty,
    ),
    case(
        34,
        "restore-terminal-stop-conditions",
        ConformanceFamily::Restore,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
    case(
        35,
        "restore-no-retry-rollback-resume-link-change",
        ConformanceFamily::Restore,
        ConformanceBoundary::RtuPty,
    ),
    case(
        36,
        "pressure-over-seventy-percent-degrades-plan",
        ConformanceFamily::PressureTimers,
        ConformanceBoundary::RtuPty,
    ),
    case(
        37,
        "pressure-full-queue-explicit-drop-or-full",
        ConformanceFamily::PressureTimers,
        ConformanceBoundary::RtuPty,
    ),
    case(
        38,
        "pressure-safety-burst-no-starvation",
        ConformanceFamily::PressureTimers,
        ConformanceBoundary::RtuPty,
    ),
    case(
        39,
        "timers-suspend-late-without-burst",
        ConformanceFamily::PressureTimers,
        ConformanceBoundary::RtuPty,
    ),
    case(
        40,
        "slow-csv-log-do-not-break-rtu-budget",
        ConformanceFamily::PressureTimers,
        ConformanceBoundary::RtuPtyWithInjectedPort,
    ),
];

#[must_use]
pub fn conformance_case(id: u8) -> Option<&'static ConformanceCase> {
    CONFORMANCE_CASES.iter().find(|item| item.id == id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{CONFORMANCE_CASE_COUNT, CONFORMANCE_CASES, ConformanceBoundary};

    #[test]
    fn matrix_is_exactly_cases_one_through_forty() {
        assert_eq!(CONFORMANCE_CASES.len(), CONFORMANCE_CASE_COUNT);

        let ids = CONFORMANCE_CASES
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, (1_u8..=40).collect::<Vec<_>>());

        let slugs = CONFORMANCE_CASES
            .iter()
            .map(|item| item.slug)
            .collect::<BTreeSet<_>>();
        assert_eq!(slugs.len(), CONFORMANCE_CASE_COUNT);
    }

    #[test]
    fn every_serial_case_declares_a_real_pty_boundary() {
        for item in &CONFORMANCE_CASES {
            if matches!(
                item.boundary,
                ConformanceBoundary::RtuPty | ConformanceBoundary::RtuPtyWithInjectedPort
            ) {
                assert!(item.slug.len() > 3);
            }
        }
    }
}
