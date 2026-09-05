use lantern_domain::{DeviceWriteOutcome, OperationToken, ParameterId};

use crate::ApprovedRestorePlan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreStepResult {
    pub index: usize,
    pub parameter_id: ParameterId,
    pub outcome: DeviceWriteOutcome,
}

/// Single-use, non-clone capability proving that the restore operation start is already durable.
///
/// Construction is crate-private and requires the exact `OperationToken` returned by `AuditPort`.
/// Holding an `ApprovedRestorePlan` alone never grants this capability.
///
/// ```compile_fail
/// use lantern_app::RestoreOperationPermit;
/// fn duplicate(permit: RestoreOperationPermit) {
///     let _copy = permit.clone();
/// }
/// ```
pub struct RestoreOperationPermit {
    token: Option<OperationToken>,
    plan: ApprovedRestorePlan,
    next_index: usize,
    results: Vec<RestoreStepResult>,
    active: bool,
}

impl RestoreOperationPermit {
    pub(crate) fn new(token: OperationToken, plan: ApprovedRestorePlan) -> Self {
        Self {
            token: Some(token),
            plan,
            next_index: 0,
            results: Vec::new(),
            active: true,
        }
    }

    #[must_use]
    pub fn plan(&self) -> &ApprovedRestorePlan {
        &self.plan
    }

    #[must_use]
    pub const fn next_index(&self) -> usize {
        self.next_index
    }

    #[must_use]
    pub fn results(&self) -> &[RestoreStepResult] {
        &self.results
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn token_matches_plan(&self) -> bool {
        self.token.as_ref().is_some_and(|token| {
            token.operation_id() == self.plan.operation_id()
                && token.backup_id() == self.plan.backup_id()
                && token.plan_hash() == self.plan.plan_hash()
                && token.session_id() == self.plan.session_id()
                && token.fingerprint() == self.plan.fingerprint()
                && token.profile_hash() == self.plan.profile_hash()
        })
    }

    pub(crate) fn record_verified_step(&mut self, outcome: DeviceWriteOutcome) {
        let step = &self.plan.steps()[self.next_index];
        self.results.push(RestoreStepResult {
            index: step.index(),
            parameter_id: step.parameter_id().clone(),
            outcome,
        });
        self.next_index = self.next_index.saturating_add(1);
    }

    pub(crate) fn record_terminal_step(
        &mut self,
        index: usize,
        parameter_id: ParameterId,
        outcome: DeviceWriteOutcome,
    ) {
        self.results.push(RestoreStepResult {
            index,
            parameter_id,
            outcome,
        });
        self.active = false;
    }

    pub(crate) fn deactivate(&mut self) {
        self.active = false;
    }

    pub(crate) fn take_token(&mut self) -> Option<OperationToken> {
        self.active = false;
        self.token.take()
    }

    pub(crate) fn into_parts(mut self) -> (Option<OperationToken>, ApprovedRestorePlan, usize, Vec<RestoreStepResult>) {
        self.active = false;
        (self.token.take(), self.plan, self.next_index, self.results)
    }
}
