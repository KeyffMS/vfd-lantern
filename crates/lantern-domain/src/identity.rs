use std::time::Duration;

use crate::{
    DeviceFingerprint, EngineeringValue, ProfileId, RawRegisters, TelemetryQuality,
};

/// Result of comparing all bounded identification probes with the available profiles.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IdentificationMatch {
    /// Every required probe matched exactly and the result is unique.
    Match,
    /// At least one probe matched, but the complete identity was not proven.
    Partial,
    /// A response was received, but required values did not match.
    Mismatch,
    /// More than one profile matched the available evidence.
    Ambiguous,
    /// Identification could not complete because a probe or transport operation failed.
    Error,
}

/// Exact result of one bounded read-only identification probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationProbeResult {
    pub probe_id: String,
    pub description: String,
    pub expected_raw: Box<[RawRegisters]>,
    pub raw: Option<RawRegisters>,
    /// Engineering representation when the identification probe declares one.
    ///
    /// Profile v1 identification probes are raw-only, so this is currently `None` rather
    /// than inventing a codec that does not exist in the validated profile.
    pub engineering: Option<EngineeringValue>,
    pub quality: TelemetryQuality,
    pub elapsed: Duration,
    pub matched: bool,
    pub error: Option<String>,
}

/// Verified identity created only after a unique complete match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDeviceIdentity {
    pub profile_id: ProfileId,
    pub fingerprint: DeviceFingerprint,
    pub probes: Box<[IdentificationProbeResult]>,
}

/// Exportable identification report retained for both successful and failed attempts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationReport {
    pub profile_id: ProfileId,
    pub outcome: IdentificationMatch,
    pub probes: Box<[IdentificationProbeResult]>,
    pub fingerprint_candidate: Option<DeviceFingerprint>,
    pub profile_hash: String,
    pub elapsed: Duration,
    pub error: Option<String>,
}
