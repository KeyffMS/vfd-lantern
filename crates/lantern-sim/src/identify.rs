use std::{path::PathBuf, time::Duration};

use lantern_app::{
    AdapterIdentity, BusError, IdentificationRequest, MonotonicClock, ReadBusPort,
    TokioMonotonicClock,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationProbeResult, IdentificationReport,
    SessionId, TelemetryQuality,
};
use lantern_profile::ValidatedDeviceProfile;

pub use lantern_app::IdentificationAttempt;

/// Simulator compatibility adapter around the production application identification use case.
///
/// The explicit scenario fingerprint is retained only for existing #20 simulator tests. The
/// real #13 composition path derives its fingerprint from opened-adapter and probe evidence in
/// `lantern-app` and does not accept an externally supplied identity.
pub async fn identify_profile_via_bus(
    bus: &dyn ReadBusPort,
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    timeout: Duration,
) -> Result<IdentificationAttempt, BusError> {
    identify_profile_via_bus_with_clock(
        bus,
        profile,
        session_id,
        fingerprint,
        timeout,
        &TokioMonotonicClock,
    )
    .await
}

pub async fn identify_profile_via_bus_with_clock(
    bus: &dyn ReadBusPort,
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    timeout: Duration,
    clock: &dyn MonotonicClock,
) -> Result<IdentificationAttempt, BusError> {
    let adapter = AdapterIdentity {
        stable_id: None,
        canonical_device: PathBuf::from("/dev/vfd-lantern-simulator"),
        vendor_id: None,
        product_id: None,
        serial_number: None,
    };
    let mut attempt = lantern_app::identify_profile_via_bus_with_clock(
        bus,
        IdentificationRequest {
            selected_profile: profile,
            candidate_profiles: &[],
            adapter: &adapter,
            session_id,
            slave_id: profile.protocol().default_link().slave_id,
            timeout,
        },
        clock,
    )
    .await;

    // #20 exposed transport failures as Result::Err. Keep that compatibility boundary for
    // simulator callers while the real #13 application path retains the richer Error report.
    if attempt.report.outcome == IdentificationMatch::Error
        && let Some(error) = legacy_transport_error(&attempt)
    {
        return Err(error);
    }

    attempt.diagnostics.fingerprint_candidate = Some(fingerprint.clone());
    if let Some(verified) = &mut attempt.verified {
        verified.device.fingerprint = fingerprint;
    }
    Ok(attempt)
}

fn legacy_transport_error(attempt: &IdentificationAttempt) -> Option<BusError> {
    let probe = attempt.diagnostics.probes.last()?;
    let message = probe.error.as_deref().unwrap_or_default();
    Some(match probe.quality {
        TelemetryQuality::Timeout if message == "request deadline expired before transmission" => {
            BusError::TimeoutBeforeSend
        }
        TelemetryQuality::Timeout => BusError::ResponseTimeout,
        TelemetryQuality::ProtocolException => BusError::ProtocolException {
            code: message
                .strip_prefix("Modbus exception ")
                .and_then(|code| code.parse().ok())
                .unwrap_or(0),
        },
        TelemetryQuality::Disconnected if message == "serial port was removed" => {
            BusError::PortRemoved
        }
        TelemetryQuality::Disconnected => BusError::Shutdown,
        TelemetryQuality::DecodeError if message == "invalid Modbus response" => {
            BusError::InvalidResponse
        }
        TelemetryQuality::DecodeError => BusError::InvalidFrameOrTransport,
        TelemetryQuality::Unavailable if message == "permission denied" => {
            BusError::PermissionDenied
        }
        TelemetryQuality::Unavailable if message == "serial port is busy" => BusError::PortBusy,
        TelemetryQuality::Unavailable if message == "request was cancelled" => BusError::Cancelled,
        TelemetryQuality::Unavailable if message == "bounded bus queue is full" => {
            BusError::QueueFull
        }
        TelemetryQuality::Unavailable if message == "write started but its outcome is unknown" => {
            BusError::OutcomeUnknown
        }
        TelemetryQuality::Unavailable => BusError::Io(message.to_owned()),
        TelemetryQuality::Good | TelemetryQuality::Stale => return None,
    })
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
