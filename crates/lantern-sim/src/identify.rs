use std::time::{Duration, Instant};

use lantern_app::{
    BusError, BusRequestContext, ReadBusPort, ReadBusRequest, RequestClass,
    VerifiedSessionIdentity,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationProbeResult, IdentificationReport,
    ModbusFunction, ModbusTable, RequestId, SessionId, VerifiedDeviceIdentity,
};
use lantern_profile::ValidatedDeviceProfile;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationAttempt {
    pub report: IdentificationReport,
    pub verified: Option<VerifiedSessionIdentity>,
}

pub async fn identify_profile_via_bus(
    bus: &dyn ReadBusPort,
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    timeout: Duration,
) -> Result<IdentificationAttempt, BusError> {
    let mut results = Vec::with_capacity(profile.probes().len());
    for (index, probe) in profile.probes().iter().enumerate() {
        let function = match probe.block.table() {
            ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
            ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        };
        let request_id = RequestId::new(u64::try_from(index).unwrap_or(u64::MAX) + 1);
        let raw = bus
            .read(ReadBusRequest {
                context: BusRequestContext {
                    request_id,
                    session_id,
                    class: RequestClass::Interactive,
                    deadline: Instant::now() + timeout,
                    operation_id: None,
                },
                slave: profile.protocol().default_link().slave_id,
                function,
                block: probe.block,
                periodic: false,
            })
            .await?;
        let matched = probe.expected_raw.iter().any(|expected| expected == &raw);
        results.push(IdentificationProbeResult {
            probe_id: probe.id.clone(),
            raw,
            matched,
        });
    }

    let matched = results.iter().filter(|probe| probe.matched).count();
    let outcome = if matched == results.len() && !results.is_empty() {
        IdentificationMatch::Match
    } else if matched > 0 {
        IdentificationMatch::Partial
    } else {
        IdentificationMatch::Mismatch
    };
    let probes = results.into_boxed_slice();
    let report = IdentificationReport {
        profile_id: profile.profile_id().clone(),
        outcome,
        probes: probes.clone(),
    };
    let verified = (outcome == IdentificationMatch::Match).then(|| VerifiedSessionIdentity {
        device: VerifiedDeviceIdentity {
            profile_id: profile.profile_id().clone(),
            fingerprint,
            probes,
        },
        profile_hash: profile.profile_hash(),
    });
    Ok(IdentificationAttempt { report, verified })
}

#[must_use]
pub fn ambiguous_identification_report(
    profile: &ValidatedDeviceProfile,
    probes: Box<[IdentificationProbeResult]>,
) -> IdentificationReport {
    IdentificationReport {
        profile_id: profile.profile_id().clone(),
        outcome: IdentificationMatch::Ambiguous,
        probes,
    }
}
