use lantern_app::restore_eligibility;
use lantern_domain::{RestoreEligibility, RestorePolicy};
use lantern_profile::{ProfileFormat, parse_and_validate_profile};
use lantern_sim::{ConformanceBoundary, conformance_case};

fn profile(policy: &str, access: &str) -> lantern_profile::ValidatedDeviceProfile {
    parse_and_validate_profile(
        format!(
            r#"schema_version = 1
profile_id = "conformance.restore.{policy}.{access}"
revision = 1
vendor = "Test"
family = "Conformance"
model = "Restore policy"
[hardware_verification]
firmware = ["test"]
method = "deterministic"
qualification_report_id = "CONFORMANCE-27"
[protocol]
default_baud_rate = 9600
allowed_baud_rates = [9600]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
[[parameters]]
id = "status.drive_state"
code = "D1"
name = "Drive state"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = 2 }}
encoding = "enum16"
quantity = "digital_state"
unit = "bool"
enum_values = {{ "0" = "Stopped", "1" = "Running", "2" = "Faulted" }}
[drive_state_source]
parameter_id = "status.drive_state"
stopped_raw = [[0]]
running_raw = [[1]]
faulted_raw = [[2]]
[[parameters]]
id = "config.value"
code = "P1"
name = "Value"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = 10 }}
encoding = "unsigned16"
quantity = "count"
unit = "count"
access = "{access}"
restore_policy = "{policy}"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = {{ kind = "accepted_raw_set", values = [[10]], documentation_source = "test", hil_report_id = "CONFORMANCE-27" }}
"#
        )
        .as_bytes(),
        ProfileFormat::Toml,
    )
    .expect("policy profile")
}

#[test]
fn case_31_non_normal_restore_policies_and_access_classes_are_not_eligible() {
    assert_eq!(
        conformance_case(31).expect("case").boundary,
        ConformanceBoundary::Domain
    );
    for (policy, expected) in [
        ("link_critical", RestoreEligibility::LinkCritical),
        ("restart_required", RestoreEligibility::RestartRequired),
        ("manual_only", RestoreEligibility::ManualOnly),
    ] {
        let profile = profile(policy, "writable_when_stopped");
        let parameter = profile
            .parameter(&lantern_domain::ParameterId::parse("config.value").expect("parameter"))
            .expect("parameter");
        assert_eq!(parameter.restore_policy(), match policy {
            "link_critical" => RestorePolicy::LinkCritical,
            "restart_required" => RestorePolicy::RestartRequired,
            "manual_only" => RestorePolicy::ManualOnly,
            _ => unreachable!(),
        });
        assert_eq!(restore_eligibility(parameter), expected);
    }

    for (access, expected) in [
        ("commissioning", RestoreEligibility::Commissioning),
        ("dangerous", RestoreEligibility::Dangerous),
    ] {
        let profile = profile("normal", access);
        let parameter = profile
            .parameter(&lantern_domain::ParameterId::parse("config.value").expect("parameter"))
            .expect("parameter");
        assert_eq!(restore_eligibility(parameter), expected);
    }
}
