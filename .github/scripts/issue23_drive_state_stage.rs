use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(140)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-profile/src/document.rs",
        "    pub fault_source: Option<FaultSourceDocumentV1>,\n",
        "    pub drive_state_source: Option<DriveStateSourceDocumentV1>,\n    pub fault_source: Option<FaultSourceDocumentV1>,\n",
    );
    replace_once(
        "crates/lantern-profile/src/document.rs",
        "#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]\n#[serde(deny_unknown_fields)]\npub struct FaultSourceDocumentV1 {\n",
        "#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]\n#[serde(deny_unknown_fields)]\npub struct DriveStateSourceDocumentV1 {\n    pub parameter_id: String,\n    #[serde(default)]\n    pub stopped_raw: Vec<Vec<u16>>,\n    #[serde(default)]\n    pub running_raw: Vec<Vec<u16>>,\n    #[serde(default)]\n    pub faulted_raw: Vec<Vec<u16>>,\n}\n\n#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]\n#[serde(deny_unknown_fields)]\npub struct FaultSourceDocumentV1 {\n",
    );

    replace_once(
        "crates/lantern-profile/src/validate/mod.rs",
        "    BaudRate, ByteOrder, DataBits, FaultSeverity, FixedScale, LinkSettings, ModbusFunction,\n",
        "    BaudRate, ByteOrder, DataBits, DriveState, FaultSeverity, FixedScale, LinkSettings, ModbusFunction,\n",
    );
    replace_once(
        "crates/lantern-profile/src/validate/mod.rs",
        "/// Validated profile fault source.\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct ValidatedFaultSource {\n",
        "/// Profile-defined, exact-raw source for the safety-critical drive state guard.\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct ValidatedDriveStateSource {\n    pub parameter_id: ParameterId,\n    stopped_raw: Box<[RawRegisters]>,\n    running_raw: Box<[RawRegisters]>,\n    faulted_raw: Box<[RawRegisters]>,\n}\n\nimpl ValidatedDriveStateSource {\n    #[must_use]\n    pub fn classify(&self, raw: &RawRegisters) -> DriveState {\n        if self.stopped_raw.iter().any(|value| value == raw) {\n            DriveState::Stopped\n        } else if self.running_raw.iter().any(|value| value == raw) {\n            DriveState::Running\n        } else if self.faulted_raw.iter().any(|value| value == raw) {\n            DriveState::Faulted\n        } else {\n            DriveState::Unknown\n        }\n    }\n}\n\n/// Validated profile fault source.\n#[derive(Clone, Debug, Eq, PartialEq)]\npub struct ValidatedFaultSource {\n",
    );
    replace_once(
        "crates/lantern-profile/src/validate/mod.rs",
        "    groups: Box<[ValidatedParameterGroup]>,\n    fault_source: Option<ValidatedFaultSource>,\n",
        "    groups: Box<[ValidatedParameterGroup]>,\n    drive_state_source: Option<ValidatedDriveStateSource>,\n    fault_source: Option<ValidatedFaultSource>,\n",
    );
    replace_once(
        "crates/lantern-profile/src/validate/mod.rs",
        "    pub fn groups(&self) -> &[ValidatedParameterGroup] {\n        &self.groups\n    }\n\n    #[must_use]\n    pub fn fault_source(&self) -> Option<&ValidatedFaultSource> {\n",
        "    pub fn groups(&self) -> &[ValidatedParameterGroup] {\n        &self.groups\n    }\n\n    #[must_use]\n    pub fn drive_state_source(&self) -> Option<&ValidatedDriveStateSource> {\n        self.drive_state_source.as_ref()\n    }\n\n    #[must_use]\n    pub fn fault_source(&self) -> Option<&ValidatedFaultSource> {\n",
    );

    replace_once(
        "crates/lantern-profile/src/lib.rs",
        "    FaultSourceKind, ReadBackPolicy, ValidatedDeviceProfile, ValidatedFaultDefinition,\n",
        "    FaultSourceKind, ReadBackPolicy, ValidatedDeviceProfile, ValidatedDriveStateSource, ValidatedFaultDefinition,\n",
    );

    replace_once(
        "crates/lantern-profile/src/validate/references/mod.rs",
        "pub(crate) fn validate_faults(\n",
        r#"pub(crate) fn validate_drive_state_source(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Option<ValidatedDriveStateSource>, ProfileError> {
    let writable_needs_stopped = parameters.values().any(|parameter| {
        matches!(
            parameter.access(),
            ParameterAccess::WritableWhenStopped | ParameterAccess::Commissioning
        ) && parameter.required_drive_state() == RequiredDriveState::Stopped
    });
    let Some(source) = document.drive_state_source.as_ref() else {
        if writable_needs_stopped {
            return Err(ProfileError::validation(
                "drive_state_source",
                "write-capable profile requires an authoritative drive-state source",
            ));
        }
        return Ok(None);
    };
    let parameter_id = ParameterId::parse(source.parameter_id.clone())
        .map_err(|error| ProfileError::validation("drive_state_source.parameter_id", error))?;
    let parameter = parameters.get(&parameter_id).ok_or_else(|| {
        ProfileError::validation(
            "drive_state_source.parameter_id",
            "references an unknown parameter",
        )
    })?;
    let width = usize::from(parameter.block().count().get());
    let mut seen = BTreeSet::<Vec<u16>>::new();
    let mut convert = |path: &str, values: &[Vec<u16>]| -> Result<Box<[RawRegisters]>, ProfileError> {
        let mut out = Vec::with_capacity(values.len());
        for (index, words) in values.iter().enumerate() {
            if words.len() != width {
                return Err(ProfileError::validation(
                    format!("drive_state_source.{path}[{index}]"),
                    format!("raw width {} does not match source parameter width {width}", words.len()),
                ));
            }
            if !seen.insert(words.clone()) {
                return Err(ProfileError::validation(
                    format!("drive_state_source.{path}[{index}]"),
                    "drive-state raw values must be unique across all classes",
                ));
            }
            out.push(RawRegisters::new(words.clone()).map_err(|error| {
                ProfileError::validation(format!("drive_state_source.{path}[{index}]"), error)
            })?);
        }
        Ok(out.into_boxed_slice())
    };
    let stopped_raw = convert("stopped_raw", &source.stopped_raw)?;
    let running_raw = convert("running_raw", &source.running_raw)?;
    let faulted_raw = convert("faulted_raw", &source.faulted_raw)?;
    if stopped_raw.is_empty() {
        return Err(ProfileError::validation(
            "drive_state_source.stopped_raw",
            "at least one exact stopped raw value is required",
        ));
    }
    Ok(Some(ValidatedDriveStateSource {
        parameter_id,
        stopped_raw,
        running_raw,
        faulted_raw,
    }))
}

pub(crate) fn validate_faults(
"#,
    );

    replace_once(
        "crates/lantern-profile/src/validate/build/profile.rs",
        "    let groups = validate_groups(&document, &parameters)?;\n    let (fault_source, faults) = validate_faults(&document, &parameters)?;\n",
        "    let groups = validate_groups(&document, &parameters)?;\n    let drive_state_source = validate_drive_state_source(&document, &parameters)?;\n    let (fault_source, faults) = validate_faults(&document, &parameters)?;\n",
    );
    replace_once(
        "crates/lantern-profile/src/validate/build/profile.rs",
        "        groups: groups.into_boxed_slice(),\n        fault_source,\n",
        "        groups: groups.into_boxed_slice(),\n        drive_state_source,\n        fault_source,\n",
    );

    replace_once(
        "profiles/example-vfd.toml",
        "[[parameters]]\nid = \"config.acceleration\"\n",
        r#"[[parameters]]
id = "status.drive_state"
code = "D1.01"
name = "Drive state"
description = "Fictional exact state guard used only by tests"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "enum16"
quantity = "digital_state"
unit = "bool"
enum_values = { "0" = "Stopped", "1" = "Running", "2" = "Faulted" }

[drive_state_source]
parameter_id = "status.drive_state"
stopped_raw = [[0]]
running_raw = [[1]]
faulted_raw = [[2]]

[[parameters]]
id = "config.acceleration"
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "        if before.drive_state != DriveState::Stopped {\n            return Err(self\n                .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)\n                .await);\n        }\n\n        let target_raw = match authoritative_target(parameter, &intent.requested_engineering) {\n",
        "        if !matches!(\n            self.read_drive_state(&profile, before.session_id, operation_id).await,\n            Ok(DriveState::Stopped)\n        ) {\n            return Err(self\n                .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)\n                .await);\n        }\n\n        let target_raw = match authoritative_target(parameter, &intent.requested_engineering) {\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "        if !same_write_context(&before, &after) || after.drive_state != DriveState::Stopped {\n",
        "        if !same_write_context(&before, &after) {\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "            || before.guard_revision != stored.guard_revision\n            || before.drive_state != DriveState::Stopped\n        {\n",
        "            || before.guard_revision != stored.guard_revision\n        {\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "        let final_old = match self\n            .read_parameter_raw(parameter, plan.session_id, plan.operation_id)\n",
        "        if !matches!(\n            self.read_drive_state(&profile, plan.session_id, plan.operation_id).await,\n            Ok(DriveState::Stopped)\n        ) {\n            return Ok(WriteOutcome::NotExecuted(\n                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)\n                    .await,\n            ));\n        }\n\n        let final_old = match self\n            .read_parameter_raw(parameter, plan.session_id, plan.operation_id)\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "            || after.guard_revision != stored.guard_revision\n            || after.drive_state != DriveState::Stopped\n        {\n",
        "            || after.guard_revision != stored.guard_revision\n        {\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "    async fn read_parameter_raw(\n",
        r#"    async fn read_drive_state(
        &mut self,
        profile: &ValidatedDeviceProfile,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<DriveState, BusError> {
        let source = profile
            .drive_state_source()
            .ok_or(BusError::InvalidRequest("profile has no authoritative drive-state source"))?;
        let parameter = profile
            .parameter(&source.parameter_id)
            .ok_or(BusError::InvalidRequest("drive-state source parameter disappeared"))?;
        let raw = self
            .read_parameter_raw(parameter, session_id, operation_id)
            .await?;
        Ok(source.classify(&raw))
    }

    async fn read_parameter_raw(
"#,
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "        && left.operation_idle == right.operation_idle\n        && left.drive_state == right.drive_state\n        && left.guard_revision == right.guard_revision\n",
        "        && left.operation_idle == right.operation_idle\n        && left.guard_revision == right.guard_revision\n",
    );
}
