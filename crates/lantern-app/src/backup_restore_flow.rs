use std::path::PathBuf;

use lantern_domain::{BackupId, MonotonicInstant};

use crate::ApprovedRestorePlan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupRuntimeMetadata {
    pub profile_origin: String,
    pub adapter: String,
    pub link_settings: String,
}

#[derive(Clone, Debug)]
pub enum BackupRestoreAction {
    CaptureBackup,
    BackupCaptured(Result<PathBuf, String>),
    PrepareRestore { source: PathBuf },
    RestorePrepared(Result<Box<ApprovedRestorePlan>, String>),
    ConfirmRestore { operator_text: String },
    CancelRestore,
    RestoreCompleted(Result<String, String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlanPresentation {
    pub backup_id: BackupId,
    pub pre_restore_backup_id: BackupId,
    pub plan_hash: String,
    pub challenge: String,
    pub step_count: usize,
    pub skipped_count: usize,
    pub expires_at: MonotonicInstant,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackupRestoreView {
    pub last_backup: Option<PathBuf>,
    pub restore_source: Option<PathBuf>,
    pub prepared_restore: Option<RestorePlanPresentation>,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ApplicationBackupRestoreState {
    pub last_backup: Option<PathBuf>,
    pub restore_source: Option<PathBuf>,
    pub prepared_restore: Option<ApprovedRestorePlan>,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl ApplicationBackupRestoreState {
    pub(crate) fn view(&self) -> BackupRestoreView {
        BackupRestoreView {
            last_backup: self.last_backup.clone(),
            restore_source: self.restore_source.clone(),
            prepared_restore: self.prepared_restore.as_ref().map(|plan| RestorePlanPresentation {
                backup_id: plan.backup_id(),
                pre_restore_backup_id: plan.pre_restore_backup_id(),
                plan_hash: plan.plan_hash().to_owned(),
                challenge: plan.challenge().to_owned(),
                step_count: plan.steps().len(),
                skipped_count: plan.skipped().len(),
                expires_at: plan.expires_at(),
            }),
            status: self.status.clone(),
            error: self.error.clone(),
        }
    }

    pub(crate) fn clear_restore(&mut self) {
        self.restore_source = None;
        self.prepared_restore = None;
    }
}
