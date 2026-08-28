use std::{fmt::Write as _, sync::Arc, time::Duration};

use lantern_domain::{
    DeviceFingerprint, EngineeringValue, IdentificationMatch, IdentificationProbeResult,
    IdentificationReport, ModbusFunction, ModbusTable, RawRegisters, RegisterBlock, RequestId,
    SessionId, SlaveId, TelemetryQuality, VerifiedDeviceIdentity,
};
use lantern_profile::ValidatedDeviceProfile;
use sha2::{Digest, Sha256};

use crate::{
    AdapterIdentity, BusError, BusRequestContext, MonotonicClock, ReadBusPort, ReadBusRequest,
    TokioMonotonicClock, VerifiedSessionIdentity,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationProbeDiagnostic {
    pub probe_id: String,
    pub description: String,
    pub block: RegisterBlock,
    pub expected_raw: Box<[RawRegisters]>,
    pub raw: Option<RawRegisters>,
    /// Profile v1 identification probes are raw-only, so this stays `None` instead of
    /// inventing an engineering codec that is absent from the validated profile.
    pub engineering: Option<EngineeringValue>,
    pub quality: TelemetryQuality,
    pub elapsed: Duration,
    pub matched: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationDiagnostics {
    pub profile_id: String,
    pub outcome: IdentificationMatch,
    pub probes: Box<[IdentificationProbeDiagnostic]>,
    pub fingerprint_candidate: Option<DeviceFingerprint>,
    pub profile_hash: String,
    pub elapsed: Duration,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationAttempt {
    /// Minimal safety-relevant report consumed by `SessionStateMachine`.
    pub report: IdentificationReport,
    pub verified: Option<VerifiedSessionIdentity>,
    /// Application-owned diagnostic evidence used by the connection wizard and offline export.
    pub diagnostics: IdentificationDiagnostics,
}

/// Performs only bounded, profile-declared read probes.
///
/// The caller supplies the already-opened adapter identity. This function performs no scanning,
/// guessing, fallback profile selection or writes.
pub async fn identify_profile_via_bus(
    bus: &dyn ReadBusPort,
    selected_profile: &ValidatedDeviceProfile,
    candidate_profiles: &[Arc<ValidatedDeviceProfile>],
    adapter: &AdapterIdentity,
    session_id: SessionId,
    slave_id: SlaveId,
    timeout: Duration,
) -> IdentificationAttempt {
    identify_profile_via_bus_with_clock(
        bus,
        selected_profile,
        candidate_profiles,
        adapter,
        session_id,
        slave_id,
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
    slave_id: SlaveId,
    timeout: Duration,
    clock: &dyn MonotonicClock,
) -> IdentificationAttempt {
    let started_at = clock.now();
    let mut core_results = Vec::with_capacity(selected_profile.probes().len());
    let mut diagnostics = Vec::with_capacity(selected_profile.probes().len());

    if selected_profile.probes().is_empty() {
        return identification_error_attempt_with_elapsed(
            selected_profile,
            Some(adapter),
            core_results,
            diagnostics,
            clock.now().saturating_duration_since(started_at),
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
            slave_id,
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
                core_results.push(IdentificationProbeResult {
                    probe_id: probe.id.clone(),
                    raw: raw.clone(),
                    matched,
                });
                diagnostics.push(IdentificationProbeDiagnostic {
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
                diagnostics.push(IdentificationProbeDiagnostic {
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
                return identification_error_attempt_with_elapsed(
                    selected_profile,
                    Some(adapter),
                    core_results,
                    diagnostics,
                    clock.now().saturating_duration_since(started_at),
                    error.to_string(),
                );
            }
        }
    }

    let matched = core_results.iter().filter(|probe| probe.matched).count();
    let mut outcome = if matched == core_results.len() {
        IdentificationMatch::Match
    } else if matched > 0 {
        IdentificationMatch::Partial
    } else {
        IdentificationMatch::Mismatch
    };

    if outcome == IdentificationMatch::Match
        && candidate_profiles.iter().any(|candidate| {
            candidate.profile_id() != selected_profile.profile_id()
                && profile_matches_observed(candidate, &diagnostics)
        })
    {
        outcome = IdentificationMatch::Ambiguous;
    }

    let fingerprint = evidence_fingerprint(selected_profile, adapter, &diagnostics);
    let core_probes = core_results.into_boxed_slice();
    let report = IdentificationReport {
        profile_id: selected_profile.profile_id().clone(),
        outcome,
        probes: core_probes.clone(),
    };
    let verified = (outcome == IdentificationMatch::Match).then(|| VerifiedSessionIdentity {
        device: VerifiedDeviceIdentity {
            profile_id: selected_profile.profile_id().clone(),
            fingerprint: fingerprint.clone(),
            probes: core_probes,
        },
        profile_hash: selected_profile.profile_hash(),
    });
    IdentificationAttempt {
        report,
        verified,
        diagnostics: IdentificationDiagnostics {
            profile_id: selected_profile.profile_id().to_string(),
            outcome,
            probes: diagnostics.into_boxed_slice(),
            fingerprint_candidate: Some(fingerprint),
            profile_hash: selected_profile.profile_hash().to_hex(),
            elapsed: clock.now().saturating_duration_since(started_at),
            error: None,
        },
    }
}

#[must_use]
pub fn identification_error_attempt(
    profile: &ValidatedDeviceProfile,
    adapter: Option<&AdapterIdentity>,
    message: impl Into<String>,
) -> IdentificationAttempt {
    identification_error_attempt_with_elapsed(
        profile,
        adapter,
        Vec::new(),
        Vec::new(),
        Duration::ZERO,
        message.into(),
    )
}

fn identification_error_attempt_with_elapsed(
    profile: &ValidatedDeviceProfile,
    adapter: Option<&AdapterIdentity>,
    core_results: Vec<IdentificationProbeResult>,
    diagnostics: Vec<IdentificationProbeDiagnostic>,
    elapsed: Duration,
    message: String,
) -> IdentificationAttempt {
    let fingerprint_candidate =
        adapter.map(|identity| evidence_fingerprint(profile, identity, &diagnostics));
    IdentificationAttempt {
        report: IdentificationReport {
            profile_id: profile.profile_id().clone(),
            outcome: IdentificationMatch::Error,
            probes: core_results.into_boxed_slice(),
        },
        verified: None,
        diagnostics: IdentificationDiagnostics {
            profile_id: profile.profile_id().to_string(),
            outcome: IdentificationMatch::Error,
            probes: diagnostics.into_boxed_slice(),
            fingerprint_candidate,
            profile_hash: profile.profile_hash().to_hex(),
            elapsed,
            error: Some(message),
        },
    }
}

fn profile_matches_observed(
    profile: &ValidatedDeviceProfile,
    observed: &[IdentificationProbeDiagnostic],
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
    probes: &[IdentificationProbeDiagnostic],
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
    let digest = digest.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    DeviceFingerprint::parse(format!("vfd:{hex}"))
        .expect("sha256 evidence fingerprint is a portable bounded identifier")
}

fn quality_for_bus_error(error: &BusError) -> TelemetryQuality {
    match error {
        BusError::TimeoutBeforeSend | BusError::ResponseTimeout => TelemetryQuality::Timeout,
        BusError::ProtocolException { .. } => TelemetryQuality::ProtocolException,
        BusError::PortRemoved | BusError::Shutdown => TelemetryQuality::Disconnected,
        BusError::InvalidFrameOrTransport | BusError::InvalidResponse => {
            TelemetryQuality::DecodeError
        }
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

    use lantern_domain::{IdentificationMatch, RawRegisters, SessionId, TelemetryQuality};
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
            profile.protocol().default_link().slave_id,
            Duration::from_secs(1),
            &clock,
        )
        .await;
        assert_eq!(attempt.report.outcome, IdentificationMatch::Match);
        assert!(attempt.diagnostics.fingerprint_candidate.is_some());
        assert!(attempt.verified.is_some());
        assert_eq!(attempt.diagnostics.probes[0].block, first.block);
    }

    #[tokio::test]
    async fn timeout_is_preserved_as_error_diagnostics_and_never_verifies() {
        let profile = profile();
        let clock = ManualMonotonicClock::default();
        let attempt = identify_profile_via_bus_with_clock(
            &StaticBus(Err(BusError::ResponseTimeout)),
            &profile,
            &[Arc::clone(&profile)],
            &adapter(),
            SessionId::new(2),
            profile.protocol().default_link().slave_id,
            Duration::from_secs(1),
            &clock,
        )
        .await;
        assert_eq!(attempt.report.outcome, IdentificationMatch::Error);
        assert!(attempt.verified.is_none());
        assert_eq!(
            attempt.diagnostics.probes[0].quality,
            TelemetryQuality::Timeout
        );
        assert!(attempt.diagnostics.probes[0].error.is_some());
    }
}
