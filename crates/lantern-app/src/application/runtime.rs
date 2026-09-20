use std::sync::Arc;

use lantern_domain::ProfileId;
use thiserror::Error;

use crate::{
    BackupRestoreAction, BackupRestoreEffect, ConnectionAction, ConnectionEffect, FaultAction,
    FaultEffect, MonitoringAction, MonitoringEffect, ParameterAction, ProfileRegistry,
    SessionEffect, SessionInput, WriteEffect,
};

use super::ApplicationState;

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Connection(ConnectionAction),
    Monitoring(MonitoringAction),
    Parameters(ParameterAction),
    Faults(FaultAction),
    BackupRestore(BackupRestoreAction),
    Session(SessionInput),
}

#[derive(Clone, Debug)]
pub enum ApplicationEffect {
    Connection(ConnectionEffect),
    Monitoring(MonitoringEffect),
    Faults(FaultEffect),
    BackupRestore(BackupRestoreEffect),
    Write(WriteEffect),
    Session(SessionEffect),
}

#[derive(Debug, Error)]
#[error("application effect failed: {0}")]
pub struct ApplicationEffectError(pub String);

pub trait EffectRunner {
    fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError>;
}

pub struct ApplicationRuntime<R> {
    state: ApplicationState,
    runner: R,
}

impl<R: EffectRunner> ApplicationRuntime<R> {
    #[must_use]
    pub fn new(state: ApplicationState, runner: R) -> Self {
        Self { state, runner }
    }

    pub fn dispatch(&mut self, action: ApplicationAction) -> Result<(), ApplicationEffectError> {
        for effect in self.state.reduce(action) {
            self.runner.execute(effect)?;
        }
        Ok(())
    }

    #[must_use]
    pub const fn state(&self) -> &ApplicationState {
        &self.state
    }
}
