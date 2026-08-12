use std::sync::Arc;

use lantern_domain::{ProfileId, SessionId};
use thiserror::Error;

use crate::{ProfileRegistry, SessionEffect, SessionInput, SessionStateMachine};

#[derive(Clone, Debug)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    registry: Arc<ProfileRegistry>,
    session: SessionStateMachine,
}

impl Default for ApplicationState {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry: Arc::new(ProfileRegistry::default()),
            session: SessionStateMachine::new(false),
        }
    }
}

impl ApplicationState {
    #[must_use]
    pub fn with_registry(registry: Arc<ProfileRegistry>, process_writes_enabled: bool) -> Self {
        Self {
            active_profile: None,
            registry,
            session: SessionStateMachine::new(process_writes_enabled),
        }
    }

    #[must_use]
    pub fn view(&self) -> ApplicationView {
        ApplicationView {
            active_profile: self.active_profile.clone(),
            active_session: self.session.session_id(),
            registry_profile_ids: self
                .registry
                .entries()
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect(),
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<ProfileRegistry> {
        &self.registry
    }

    #[must_use]
    pub const fn session(&self) -> &SessionStateMachine {
        &self.session
    }

    pub fn reduce(&mut self, action: ApplicationAction) -> Vec<ApplicationEffect> {
        match action {
            ApplicationAction::ReplaceRegistry(registry) => {
                self.registry = registry;
                Vec::new()
            }
            ApplicationAction::SelectProfile(profile_id) => {
                self.active_profile = Some(profile_id);
                Vec::new()
            }
            ApplicationAction::Session(input) => self
                .session
                .transition(input)
                .into_iter()
                .map(ApplicationEffect::Session)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationView {
    active_profile: Option<ProfileId>,
    active_session: Option<SessionId>,
    registry_profile_ids: Vec<String>,
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

    #[must_use]
    pub fn registry_profile_ids(&self) -> &[String] {
        &self.registry_profile_ids
    }
}

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Session(SessionInput),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationEffect {
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

#[cfg(test)]
mod tests {
    use crate::{ApplicationAction, ApplicationEffect, ApplicationState, EffectRunner};

    use super::{ApplicationEffectError, ApplicationRuntime};

    #[derive(Default)]
    struct RecordingRunner(Vec<ApplicationEffect>);

    impl EffectRunner for RecordingRunner {
        fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError> {
            self.0.push(effect);
            Ok(())
        }
    }

    #[test]
    fn application_runtime_is_the_only_effect_execution_boundary() {
        let mut runtime =
            ApplicationRuntime::new(ApplicationState::default(), RecordingRunner::default());
        runtime
            .dispatch(ApplicationAction::Session(crate::SessionInput::Shutdown))
            .expect("dispatch");
        assert!(matches!(
            runtime.state().session().state(),
            crate::SessionState::ShuttingDown
        ));
    }
}
