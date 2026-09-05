use std::path::PathBuf;

use lantern_domain::{PlanId, WriteIntent};

use crate::{
    ApprovedRestorePlan, BackupRuntimeMetadata, RestoreConfirmation, WriteConfirmation,
    WriteSessionSnapshot,
};

/// Application-owned effects for the production guarded-write and backup/restore boundary. The
/// composition root is the only layer allowed to turn these effects into physical bus access or
/// durable filesystem artifacts.
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
    CaptureBackup {
        metadata: BackupRuntimeMetadata,
        snapshot: WriteSessionSnapshot,
    },
    PrepareRestore {
        source: PathBuf,
        metadata: BackupRuntimeMetadata,
        snapshot: WriteSessionSnapshot,
    },
    BeginRestore {
        plan: ApprovedRestorePlan,
        confirmation: RestoreConfirmation,
        snapshot: WriteSessionSnapshot,
    },
}
