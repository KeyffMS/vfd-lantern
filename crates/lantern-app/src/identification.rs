use std::{sync::Arc, time::Duration};

use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationProbeResult, IdentificationReport,
    ModbusFunction, ModbusTable, RequestId, SessionId, TelemetryQuality, VerifiedDeviceIdentity,
};
use lantern_profile::ValidatedDeviceProfile;
use sha2::{Digest, Sha256};

use crate::{
    AdapterIdentity, BusError, BusRequestContext, MonotonicClock, ReadBusPort, ReadBusRequest,
    TokioMonotonicClock, VerifiedSessionIdentity,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationAttempt {
    pub report: IdentificationReport,
    pub verified: Option<VerifiedSessionIdentity>,
}

/// Performs only the bounded, profile-declared read probes required for identification.
///
/// The caller supplies the already-opened adapter identity. No scanning, guessing, fallback
/// profile selection or writes are performed here.
pub async fn identify_profile_via_bus(
    bus: &dyn ReadBusPort,
    selected_profile: &ValidatedDeviceProfile,
    candidate_profiles: &[Arc<ValidatedDeviceProfile>],
    adapter: &AdapterIdentity,
    session_id: SessionId,
    timeout: Duration,
) -> IdentificationAttempt {
    identify_profile_via_bus_with_clock(
        bus,
        selected_profile,
        candidate_profiles,
        adapter,
        session_id,
        timeout,
        &TokioMonotonicClock,
    )
    .await
}

pub async fn identify_profile_via_bus_with_clock(
    bus: &dyn ReadBusPort,
    selected_profile: &ValidatedDeviceProfile,
    candidate_profiles: &[Arc<ValidatedDeviceProfile>],
    adapter: &AdapterIdentity,
    session_id: SessionId,
    timeout: Duration,
    clock: &dyn MonotonicClock,
) -> IdentificationAttempt {
    let started_at = clock.now();
    let mut results = Vec::with_capacity(selected_profile.probes().len());

    if selected_profile.probes().is_empty() {
        return error_attempt(
            selected_profile,
            adapter,
            results,
            started_at.elapsed(),
            "selected profile has no identification probes".to_owned(),
        );
    }

    for (index, probe) in selected_profile.probes().iter().enumerate() {
        let function = match probe.block.table() {
            ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
            ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        };
        let request_id = RequestId::new(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1));
        let probe_started_at = clock.now();
        let request = ReadBusRequest::one_shot(
            BusRequestContext::interactive(request_id, session_id, clock.now() + timeout, None),
            selected_profile.protocol().default_link().slave_id,
            function,
            probe.block,
        );
        let raw = match request {
            Ok(request) => bus.read(request).await,
            Err(error) => Err(error),
        };

        match raw {
            Ok(raw) => {
                let matched = probe.expected_raw.iter().any(|expected| expected == &raw);
                results.push(IdentificationProbeResult {
                    probe_id: probe.id.clone(),
                    description: probe.description.clone(),
                    block: probe.block,
                    expected_raw: probe.expected_raw.to_vec().into_boxed_slice(),
                    raw: Some(raw),
                    engineering: None,
                    quality: TelemetryQuality::Good,
                    elapsed: clock.now().saturating_duration_since(probe_started_at),
                    matched,
                    error: None,
                });
            }
            Err(error) => {
                results.push(IdentificationProbeResult {
                    probe_id: probe.id.clone(),
                    description: probe.description.clone(),
                    block: probe.block,
                    expected_raw: probe.expected_raw.to_vec().into_boxed_slice(),
                    raw: None,
                    engineering: None,
                    quality: quality_for_bus_error(&error),
                    elapsed: clock.now().saturating_duration_since(probe_started_at),
                    matched: false,
                    error: Some(error.to_string()),
                });
                return error_attempt(
                    selected_profile,
                    adapter,
                    results,
                    clock.now().saturating_duration_since(started_at),
                    error.to_string(),
                );
            }
        }
    }

    let matched = results.iter().filter(|probe| probe.matched).count();
    let mut outcome = if matched == results.len() {
        IdentificationMatch::Match
    } else if matched > 0 {
        IdentificationMatch::Partial
    } else {
        IdentificationMatch::Mismatch
    };

    if outcome == IdentificationMatch::Match
        && candidate_profiles.iter().any(|candidate| {
            candidate.profile_id() != selected_profile.profile_id()
                && profile_matches_observed(candidate, &results)
        })
    {
        outcome = IdentificationMatch::Ambiguous;
    }

    let fingerprint = evidence_fingerprint(selected_profile, adapter, &results);
    let probes = results.into_boxed_slice();
    let report = IdentificationReport {
        profile_id: selected_profile.profile_id().clone(),
        outcome,
        probes: probes.clone(),
        fingerprint_candidate: Some(fingerprint.clone()),
        profile_hash: selected_profile.profile_hash().to_hex(),
        elapsed: clock.now().saturating_duration_since(started_at),
        error: None,
    };
    let verified = (outcome == IdentificationMatch::Match).then(|| VerifiedSessionIdentity {
        device: VerifiedDeviceIdentity {
            profile_id: selected_profile.profile_id().clone(),
            fingerprint,
            probes,
        },
        profile_hash: selected_profile.profile_hash(),
    });
    IdentificationAttempt { report, verified }
}

#[must_use]
pub fn identification_error_report(
    profile: &ValidatedDeviceProfile,
    adapter: Option<&AdapterIdentity>,
    message: impl Into<String>,
) -> IdentificationReport {
    let message = message.into();
    IdentificationReport {
        profile_id: profile.profile_id().clone(),
        outcome: IdentificationMatch::Error,
        probes: Box::new([]),
        fingerprint_candidate: adapter.map(|identity| evidence_fingerprint(profile, identity, &[])),
        profile_hash: profile.profile_hash().to_hex(),
        elapsed: Duration::ZERO,
        error: Some(message),
    }
}

fn error_attempt(
    profile: &ValidatedDeviceProfile,
    adapter: &AdapterIdentity,
    results: Vec<IdentificationProbeResult>,
    elapsed: Duration,
    message: String,
) -> IdentificationAttempt {
    let fingerprint = evidence_fingerprint(profile, adapter, &results);
    IdentificationAttempt {
        report: IdentificationReport {
            profile_id: profile.profile_id().clone(),
            outcome: IdentificationMatch::Error,
            probes: results.into_boxed_slice(),
            fingerprint_candidate: Some(fingerprint),
            profile_hash: profile.profile_hash().to_hex(),
            elapsed,
            error: Some(message),
        },
        verified: None,
    }
}

fn profile_matches_observed(
    profile: &ValidatedDeviceProfile,
    observed: &[IdentificationProbeResult],
) -> bool {
    !profile.probes().is_empty()
        && profile.probes().len() == observed.len()
        && profile.probes().iter().all(|candidate_probe| {
            observed.iter().any(|actual| {
                actual.block == candidate_probe.block
                    && actual.raw.as_ref().is_some_and(|raw| {
                        candidate_probe
                            .expected_raw
                            .iter()
                            .any(|expected| expected == raw)
                    })
            })
        })
}

fn evidence_fingerprint(
    profile: &ValidatedDeviceProfile,
    adapter: &AdapterIdentity,
    probes: &[IdentificationProbeResult],
) -> DeviceFingerprint {
    let mut digest = Sha256::new();
    digest.update(profile.profile_hash().to_hex().as_bytes());
    if let Some(stable_id) = &adapter.stable_id {
        digest.update(stable_id.to_string_lossy().as_bytes());
    }
    digest.update(adapter.canonical_device.to_string_lossy().as_bytes());
    if let Some(vendor_id) = adapter.vendor_id {
        digest.update(vendor_id.to_be_bytes());
    }
    if let Some(product_id) = adapter.product_id {
        digest.update(product_id.to_be_bytes());
    }
    if let Some(serial) = &adapter.serial_number {
        digest.update(serial.as_bytes());
    }
    for probe in probes {
        digest.update(probe.probe_id.as_bytes());
        if let Some(raw) = &probe.raw {
            for word in raw.as_slice() {
                digest.update(word.to_be_bytes());
            }
        }
    }
    let hex = format!("{:x}", digest.finalize());
    DeviceFingerprint::parse(format!("vfd:{hex}"))
        .expect("sha256 evidence fingerprint is a portable bounded identifier")
}

const fn quality_for_bus_error(error: &BusError) -> TelemetryQuality {
    match error {
        BusError::TimeoutBeforeSend | BusError::ResponseTimeout => TelemetryQuality::Timeout,
        BusError::ProtocolException { .. } => TelemetryQuality::ProtocolException,
        BusError::PortRemoved | BusError::Shutdown => TelemetryQuality::Disconnected,
        BusError::InvalidFrameOrTransport | BusError::InvalidResponse => TelemetryQuality::DecodeError,
        BusError::PermissionDenied
        | BusError::PortBusy
        | BusError::Io(_)
        | BusError::Cancelled
        | BusError::QueueFull
        | BusError::OutcomeUnknown
        | BusError::InvalidRequest(_) => TelemetryQuality::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use lantern_domain::{
        IdentificationMatch, RawRegisters, RegisterBlock, RequestId, SessionId,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{
        AdapterIdentity, BusError, BusFuture, ManualMonotonicClock, ReadBusPort, ReadBusRequest,
    };

    use super::identify_profile_via_bus_with_clock;

    #[derive(Clone)]
    struct StaticBus(Result<RawRegisters, BusError>);

    impl ReadBusPort for StaticBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            let result = self.0.clone();
            Box::pin(async move { result })
        }
    }

    fn profile() -> Arc<lantern_profile::ValidatedDeviceProfile> {
        Arc::new(
            parse_and_validate_profile(
                include_bytes!("../../../profiles/example-vfd.toml"),
                ProfileFormat::Toml,
            )
            .expect("profile"),
        )
    }

    fn adapter() -> AdapterIdentity {
        AdapterIdentity {
            stable_id: None,
            canonical_device: PathBuf::from("/dev/pts/7"),
            vendor_id: None,
            product_id: None,
            serial_number: None,
        }
    }

    #[tokio::test]
    async fn successful_probe_creates_verified_identity_from_observed_evidence() {
        let profile = profile();
        let first = profile.probes().first().expect("probe");
        let raw = first.expected_raw.first().expect("expected").clone();
        let clock = ManualMonotonicClock::default();
        let attempt = identify_profile_via_bus_with_clock(
            &StaticBus(Ok(raw)),
            &profile,
            &[Arc::clone(&profile)],
            &adapter(),
            SessionId::new(1),
            Duration::from_secs(1),
            &clock,
        )
        .await;
        assert_eq!(attempt.report.outcome, IdentificationMatch::Match);
        assert!(attempt.report.fingerprint_candidate.is_some());
        assert!(attempt.verified.is_some());
        assert_eq!(attempt.report.probes[0].block, first.block);
        let _ = (RequestId::new(1), RegisterBlock::clone(&first.block));
    }

    #[tokio::test]
    async fn timeout_is_preserved_as_error_report_and_never_verifies() {
        let profile = profile();
        let clock = ManualMonotonicClock::default();
        let attempt = identify_profile_via_bus_with_clock(
            &StaticBus(Err(BusError::ResponseTimeout)),
            &profile,
            &[Arc::clone(&profile)],
            &adapter(),
            SessionId::new(2),
            Duration::from_secs(1),
            &clock,
        )
        .await;
        assert_eq!(attempt.report.outcome, IdentificationMatch::Error);
        assert!(attempt.verified.is_none());
        assert!(attempt.report.probes[0].error.is_some());
    }
}
