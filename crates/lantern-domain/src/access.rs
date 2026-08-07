/// Parameter access class.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ParameterAccess {
    ReadOnly,
    WritableWhenStopped,
    Commissioning,
    Dangerous,
}

/// Restore policy assigned explicitly by a validated profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RestorePolicy {
    Normal,
    LinkCritical,
    RestartRequired,
    ManualOnly,
}

/// Current drive state derived from profile-defined telemetry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DriveState {
    Stopped,
    Running,
    Faulted,
    Unknown,
}

/// Drive-state guard required by an operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RequiredDriveState {
    Any,
    Stopped,
    Faulted,
}

impl RequiredDriveState {
    /// Checks whether a fresh state satisfies the guard.
    #[must_use]
    pub const fn is_satisfied_by(self, state: DriveState) -> bool {
        match self {
            Self::Any => true,
            Self::Stopped => matches!(state, DriveState::Stopped),
            Self::Faulted => matches!(state, DriveState::Faulted),
        }
    }
}
