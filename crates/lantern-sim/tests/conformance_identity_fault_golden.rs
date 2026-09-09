#![cfg(feature = "test-support")]

include!("support/conformance_identity_case9_golden_generate.inc");

mod faults {
    include!("support/conformance_faults_pty.inc");
    include!("support/conformance_faults_golden_generate.inc");
}
