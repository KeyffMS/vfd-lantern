//! Application state, use cases, and outbound ports.

#![forbid(unsafe_code)]

mod bus;
mod ports;
mod profile_registry;
mod serial;
mod settings;
mod write_coordinator;

use lantern_domain::{ProfileId, SessionId};

pub use bus::*;
pub use ports::*;
pub use profile_registry::*;
pub use serial::*;
pub use settings::*;
pub use write_coordinator::*;

/// Read-only application state rendered by the terminal frontend.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    active_session: Option<SessionId>,
}

impl ApplicationState {
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
    #[must_use]
    pub fn active_profile_id(&self) -> Option<&str> {
        self.active_profile.as_ref().map(ProfileId::as_str)
    }

    #[must_use]
    pub const fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }
}

/// Application-owned polling policy placeholder introduced fully by issue #10.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PollPlanner;
