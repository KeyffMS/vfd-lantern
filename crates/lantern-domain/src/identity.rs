use crate::{DeviceFingerprint, ProfileId, RawRegisters};

/// Result of comparing read-only identification probes with a profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IdentificationMatch {
    Match,
    Partial,
    Mismatch,
    Ambiguous,
    Error,
}

/// Core domain result of one read-only identification probe.
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

/// Minimal safety-relevant identification report retained by the session state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationReport {
    pub profile_id: ProfileId,
    pub outcome: IdentificationMatch,
    pub probes: Box<[IdentificationProbeResult]>,
}
