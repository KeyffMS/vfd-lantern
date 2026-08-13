use crate::{DeviceFingerprint, ProfileId, RawRegisters};

/// Result of comparing all bounded identification probes with one profile.
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
}

/// Exact result of one read-only identification probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationProbeResult {
    pub probe_id: String,
    pub raw: RawRegisters,
    pub matched: bool,
}

/// Verified identity created only after a unique complete match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDeviceIdentity {
    pub profile_id: ProfileId,
    pub fingerprint: DeviceFingerprint,
    pub probes: Box<[IdentificationProbeResult]>,
}

/// Exportable identification report retained after failed identification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationReport {
    pub profile_id: ProfileId,
    pub outcome: IdentificationMatch,
    pub probes: Box<[IdentificationProbeResult]>,
}
