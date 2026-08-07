//! Application state, use cases, and outbound ports.

#![forbid(unsafe_code)]

use lantern_domain::{ProfileId, SessionId};
use lantern_profile::ValidatedDeviceProfile;

/// Read-only application state rendered by the terminal frontend.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    active_session: Option<SessionId>,
}

impl ApplicationState {
    /// Returns a presentation-safe snapshot.
    #[must_use]
    pub fn view(&self) -> ApplicationView {
        ApplicationView {
            active_profile: self.active_profile.clone(),
            active_session: self.active_session,
        }
    }
}

/// Immutable projection exposed to presentation adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationView {
    active_profile: Option<ProfileId>,
    active_session: Option<SessionId>,
}

impl ApplicationView {
    /// Returns the active profile identifier as presentation text.
    #[must_use]
    pub fn active_profile_id(&self) -> Option<&str> {
        self.active_profile.as_ref().map(ProfileId::as_str)
    }

    /// Returns the active logical session.
    #[must_use]
    pub const fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }
}

/// Single application-owned registry snapshot.
#[derive(Clone, Debug, Default)]
pub struct ProfileRegistry {
    profiles: Vec<ValidatedDeviceProfile>,
}

impl ProfileRegistry {
    /// Builds an immutable registry snapshot.
    #[must_use]
    pub fn new(profiles: Vec<ValidatedDeviceProfile>) -> Self {
        Self { profiles }
    }

    /// Returns all profiles in deterministic registry order.
    #[must_use]
    pub fn profiles(&self) -> &[ValidatedDeviceProfile] {
        &self.profiles
    }
}

/// Application-owned polling policy placeholder.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PollPlanner;

/// Capability for read-only bus operations.
pub trait ReadBusPort: Send + Sync {
    /// Human-readable adapter name for diagnostics.
    fn adapter_name(&self) -> &'static str;
}

/// Capability for guarded write operations.
pub trait WriteBusPort: Send + Sync {
    /// Human-readable adapter name for diagnostics.
    fn adapter_name(&self) -> &'static str;
}

/// Capability for passive serial-port discovery.
pub trait PortDiscoveryPort: Send + Sync {
    /// Returns the number of currently known descriptors without opening them.
    fn known_port_count(&self) -> usize;
}

/// Source of bounded profile documents.
pub trait ProfileSourcePort: Send + Sync {
    /// Returns a stable source description.
    fn source_name(&self) -> &'static str;
}

/// Capability for user-facing persistent artifacts.
pub trait ArtifactStoragePort: Send + Sync {
    /// Returns a stable storage description.
    fn storage_name(&self) -> &'static str;
}

/// Durable audit capability used by guarded operations.
pub trait AuditPort: Send + Sync {
    /// Reports whether durable audit is available.
    fn is_available(&self) -> bool;
}

/// Profile-origin and local-approval capability.
pub trait ProfileTrustPort: Send + Sync {
    /// Reports whether a profile hash is trusted for guarded operations.
    fn is_trusted(&self, profile_id: &ProfileId) -> bool;
}

/// Time source used by deterministic application logic.
pub trait ClockPort: Send + Sync {
    /// Returns monotonic nanoseconds from an implementation-defined epoch.
    fn monotonic_ns(&self) -> u128;
}
