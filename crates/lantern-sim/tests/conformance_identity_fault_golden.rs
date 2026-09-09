#![cfg(feature = "test-support")]

macro_rules! matches {
    ($expression:expr, SessionState::Faulted { .. }) => {
        match $expression {
            SessionState::Active(active) => {
                ::core::matches!(active.connectivity, Connectivity::Faulted { .. })
            }
            _ => false,
        }
    };
    ($($tokens:tt)*) => {
        ::core::matches!($($tokens)*)
    };
}

include!("support/conformance_identity_case9_golden_generate.inc");

mod faults {
    #![allow(unused_imports)]

    use lantern_domain::SlaveId;

    include!("support/conformance_faults_pty.inc");
    include!("support/conformance_faults_golden_generate_v2.inc");
}
