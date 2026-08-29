use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_app::{
    FrequencyClass, MonitoringError, PackagedProfilesManifestV1, ParameterId, PollPlanner,
    PollPlannerConfig, ProfileRegistry, ProfileSource, ProfileSourceFormat, ProfileSourceTier,
    ReadSubscription, ScopePanel, ScopeSelection, SubscriberId, SubscriptionReason,
    monitoring_catalog, search_monitoring_catalog,
};

const PROFILE_HEADER: &str = r#"schema_version = 1
profile_id = "test.scope-acceptance"
revision = 1
vendor = "Test"
family = "Scope"
model = "Acceptance"

[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
"#;

fn registry(source: String, path: &str) -> Arc<ProfileRegistry> {
    Arc::new(
        ProfileRegistry::from_sources(
            vec![ProfileSource {
                path: PathBuf::from(path),
                bytes: source.into_bytes().into_boxed_slice(),
                format: ProfileSourceFormat::Toml,
                tier: ProfileSourceTier::Explicit,
            }],
            &PackagedProfilesManifestV1 {
                schema_version: 1,
                build_id: "issue-14-acceptance".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("acceptance profile registry"),
    )
}

fn limit_profile_source() -> String {
    let mut source = PROFILE_HEADER.to_owned();
    for index in 1_u16..=9 {
        let name = if index == 1 {
            "Shared label".to_owned()
        } else {
            format!("Channel {index}")
        };
        write!(
            source,
            r#"
[[parameters]]
id = "p{index}"
code = "P{index}"
name = "{name}"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = {} }}
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = {{ multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }}
"#,
            index - 1,
        )
        .expect("append frequency parameter");
    }
    source.push_str(
        r#"
[[parameters]]
id = "speed"
code = "RPM"
name = "Shared label"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 20 }
encoding = "unsigned16"
quantity = "rotational_speed"
unit = "rpm"
scale = { multiplier = "1", divisor = "1", offset = "0", decimal_places = 0 }
"#,
    );
    source
}

fn alias_profile_source(aliases: &[(&str, &str)]) -> String {
    let mut source = PROFILE_HEADER
        .replace("test.scope-acceptance", "test.scope-aliases")
        .replace("model = \"Acceptance\"", "model = \"Aliases\"");
    if !aliases.is_empty() {
        source.push_str("\n[aliases]\n");
        for (alias, target) in aliases {
            writeln!(source, "{alias} = \"{target}\"").expect("append alias");
        }
    }
    source.push_str(
        r#"
[[parameters]]
id = "a"
code = "A"
name = "First"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[[parameters]]
id = "b"
code = "B"
name = "Second"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }
"#,
    );
    source
}

#[test]
fn scope_accepts_exactly_eight_channels_across_four_panels_and_rejects_ninth() {
    let registry = registry(limit_profile_source(), "scope-limits.toml");
    let profile = registry
        .entries()
        .values()
        .next()
        .expect("profile")
        .profile();
    let mut scope = ScopeSelection::default();

    for index in 1_u8..=8 {
        let parameter_id = ParameterId::parse(format!("p{index}")).expect("parameter id");
        let panel = ScopePanel::new(((index - 1) / 2) + 1).expect("panel");
        assert!(scope.add(profile, parameter_id, panel).expect("channel"));
    }

    assert_eq!(scope.channels().len(), 8);
    for panel_number in 1_u8..=4 {
        let panel = ScopePanel::new(panel_number).expect("panel");
        assert_eq!(
            scope
                .channels()
                .iter()
                .filter(|channel| channel.panel() == panel)
                .count(),
            2
        );
        let groups = scope.axis_groups(profile, panel);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].parameters.len(), 2);
    }

    let ninth = ParameterId::parse("p9").expect("ninth parameter");
    assert_eq!(
        scope.add(profile, ninth, ScopePanel::new(1).expect("panel")),
        Err(MonitoringError::TooManyScopeChannels)
    );
}

#[test]
fn identical_labels_do_not_merge_different_quantity_unit_axes() {
    let registry = registry(limit_profile_source(), "scope-axis-labels.toml");
    let profile = registry
        .entries()
        .values()
        .next()
        .expect("profile")
        .profile();
    let catalog = monitoring_catalog(profile);
    let frequency = catalog
        .iter()
        .find(|parameter| parameter.parameter_id.as_str() == "p1")
        .expect("frequency");
    let speed = catalog
        .iter()
        .find(|parameter| parameter.parameter_id.as_str() == "speed")
        .expect("speed");
    assert_eq!(frequency.name, speed.name);
    assert_ne!(frequency.axis, speed.axis);

    let panel = ScopePanel::new(1).expect("panel");
    let mut scope = ScopeSelection::default();
    assert!(
        scope
            .add(profile, frequency.parameter_id.clone(), panel)
            .expect("frequency channel")
    );
    assert_eq!(
        scope.add(profile, speed.parameter_id.clone(), panel),
        Err(MonitoringError::IncompatiblePanelAxis(1))
    );
}

#[test]
fn full_partial_and_zero_alias_sets_keep_catalog_semantic() {
    let cases: [(&[(&str, &str)], usize, usize); 3] = [
        (&[("first_alias", "a"), ("second_alias", "b")], 1, 1),
        (&[("first_alias", "a")], 1, 0),
        (&[], 0, 0),
    ];

    for (aliases, expected_a, expected_b) in cases {
        let registry = registry(alias_profile_source(aliases), "scope-aliases.toml");
        let profile = registry
            .entries()
            .values()
            .next()
            .expect("profile")
            .profile();
        let catalog = monitoring_catalog(profile);
        let a = catalog
            .iter()
            .find(|parameter| parameter.parameter_id.as_str() == "a")
            .expect("a");
        let b = catalog
            .iter()
            .find(|parameter| parameter.parameter_id.as_str() == "b")
            .expect("b");
        assert_eq!(a.aliases.len(), expected_a);
        assert_eq!(b.aliases.len(), expected_b);
        assert_eq!(search_monitoring_catalog(profile, "First").len(), 1);
        assert_eq!(search_monitoring_catalog(profile, "hz").len(), 2);
        if expected_a == 1 {
            assert_eq!(search_monitoring_catalog(profile, "first_alias").len(), 1);
        } else {
            assert!(search_monitoring_catalog(profile, "first_alias").is_empty());
        }
    }
}

#[test]
fn dashboard_scope_and_csv_share_one_physical_poll_demand() {
    let registry = registry(limit_profile_source(), "scope-deduplication.toml");
    let profile = registry
        .entries()
        .values()
        .next()
        .expect("profile")
        .profile();
    let parameter_id = ParameterId::parse("p1").expect("parameter");
    let subscriptions = vec![
        ReadSubscription::new(
            parameter_id.clone(),
            FrequencyClass::Normal,
            SubscriberId::parse("dashboard:p1").expect("subscriber"),
            SubscriptionReason::Dashboard,
            false,
            Duration::from_secs(2),
        )
        .expect("dashboard subscription"),
        ReadSubscription::new(
            parameter_id.clone(),
            FrequencyClass::Fast,
            SubscriberId::parse("scope:p1").expect("subscriber"),
            SubscriptionReason::Scope,
            true,
            Duration::from_millis(500),
        )
        .expect("scope subscription"),
        ReadSubscription::new(
            parameter_id,
            FrequencyClass::Normal,
            SubscriberId::parse("csv:p1").expect("subscriber"),
            SubscriptionReason::Csv,
            false,
            Duration::from_secs(1),
        )
        .expect("csv subscription"),
    ];

    let plan = PollPlanner::new()
        .build(
            profile,
            subscriptions,
            PollPlannerConfig::for_profile(profile),
            Instant::now(),
        )
        .expect("poll plan");
    assert_eq!(plan.blocks().len(), 1);
    assert_eq!(plan.blocks()[0].parameters().len(), 1);
    assert_eq!(plan.blocks()[0].parameters()[0].subscribers().len(), 3);
    assert!(plan.blocks()[0].parameters()[0].history_required());
    assert_eq!(plan.blocks()[0].period(), Duration::from_millis(100));
    assert_eq!(
        plan.blocks()[0].maximum_age(),
        Duration::from_millis(500)
    );
}
