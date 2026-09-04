use lantern_domain::{PlanId, WriteIntent};

use crate::{WriteConfirmation, WriteSessionSnapshot};

/// Application-owned effects for the production guarded-write boundary. The composition root is
/// the only layer allowed to turn these effects into access to a physical bus.
#[derive(Clone, Debug)]
pub enum WriteEffect {
    SyncSession(WriteSessionSnapshot),
    Prepare {
        intent: WriteIntent,
        snapshot: WriteSessionSnapshot,
    },
    Confirm {
        plan_id: PlanId,
        confirmation: WriteConfirmation,
        snapshot: WriteSessionSnapshot,
    },
    Cancel {
        plan_id: PlanId,
    },
}
