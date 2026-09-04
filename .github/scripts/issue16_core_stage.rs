use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(120)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-app/src/bus.rs",
        r#"pub trait WriteBusPort: Send + Sync {
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()>;
}
"#,
        r#"pub trait WriteBusPort: Send + Sync {
    /// Executes the single capability minted by the private write kernel.
    fn execute(&self, request: PreparedBusWrite) -> BusFuture<'static, ()>;

    /// Compatibility alias for lower-level tests. Production write orchestration calls `execute`.
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()> {
        self.execute(request)
    }
}
"#,
    );

    replace_once(
        "crates/lantern-transport/src/bus_actor.rs",
        r#"impl WriteBusPort for BusActorHandle {
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()> {
"#,
        r#"impl WriteBusPort for BusActorHandle {
    fn execute(&self, request: PreparedBusWrite) -> BusFuture<'static, ()> {
"#,
    );

    replace_once(
        "crates/lantern-app/src/session.rs",
        r#"use lantern_domain::{
    IdentificationMatch, IdentificationReport, OperationId, PlanId, SessionId,
    VerifiedDeviceIdentity, WriteOutcome,
};
"#,
        r#"use lantern_domain::{
    DecisionOutcome, DeviceWriteOutcome, IdentificationMatch, IdentificationReport, OperationId,
    PlanId, SessionId, VerifiedDeviceIdentity, WriteOutcome,
};
"#,
    );

    replace_once(
        "crates/lantern-app/src/session.rs",
        r#"                match outcome {
                    WriteOutcome::OutcomeUnknown => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::OutcomeUnknown,
                        );
                    }
                    WriteOutcome::AuditDegraded => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::AuditDegraded,
                        );
                        active.audit_health = AuditHealth::Degraded {
                            cause: "write audit finalization failed".to_owned(),
                            since: now,
                        };
                    }
                    _ => {}
                }
"#,
        r#"                match outcome {
                    WriteOutcome::Executed(
                        DeviceWriteOutcome::OutcomeUnknown | DeviceWriteOutcome::TransportLost,
                    ) => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::OutcomeUnknown,
                        );
                    }
                    WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
                    | WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable) => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::AuditDegraded,
                        );
                        active.audit_health = AuditHealth::Degraded {
                            cause: "write audit finalization failed".to_owned(),
                            since: now,
                        };
                    }
                    _ => {}
                }
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
"#,
        r#"    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"        DeviceFingerprint, DeviceWriteOutcome, DriveState, EngineeringValue, ModbusFunction,
        ModbusTable, OperationId, ParameterId, RawRegisters, RegisterAddress, RegisterBlock,
"#,
        r#"        DeviceFingerprint, DeviceWriteOutcome, DriveState, ModbusFunction, ModbusTable,
        OperationId, ParameterId, RawRegisters, RegisterAddress, RegisterBlock,
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"        AuditPort, BusError, BusFuture, ClockPort, PortFuture, ProfileTrustError, ProfileTrustPort,
"#,
        r#"        AuditPort, BusFuture, ClockPort, PortFuture, ProfileTrustError, ProfileTrustPort,
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"        if !authority.operation_step_is_well_formed()
            || profile.profile_hash().to_hex() != authority.profile_hash()
        {
"#,
        r#"        if !authority.operation_step_is_well_formed()
            || authority.context_hash().is_empty()
            || authority.fingerprint().as_str().is_empty()
            || profile.profile_hash().to_hex() != authority.profile_hash()
        {
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"        if !manual_parameter_allowed(parameter)
            || parameter
                .forbidden_raw()
"#,
        r#"        if authority.expected_old_raw().as_slice().len()
            != usize::from(parameter.block().count().get())
            || !manual_parameter_allowed(parameter)
            || parameter
                .forbidden_raw()
"#,
    );

    replace_once(
        "scripts/check-architecture.sh",
        r#"cargo metadata --locked --no-deps --format-version 1 >/dev/null
"#,
        r#"if grep -R -n -E '\b(WriteCoordinator|PreparedBusWrite)\b' crates/vfd-lantern/src; then
    printf 'production composition root exposes guarded writes before #22/#23\n' >&2
    exit 1
fi

cargo metadata --locked --no-deps --format-version 1 >/dev/null
"#,
    );
}
