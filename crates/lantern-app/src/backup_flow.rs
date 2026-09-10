use std::path::PathBuf;

use lantern_domain::{BackupDiffStatus, BackupDifference, BackupSnapshot, DeviceWriteOutcome};

use crate::{ApprovedRestorePlan, BackupCaptureContext, RestoreConfirmation};

#[derive(Clone, Debug)]
pub struct StoredBackup {
    pub path: PathBuf,
    pub snapshot: BackupSnapshot,
}

#[derive(Clone, Debug)]
pub struct PreparedRestoreBundle {
    pub pre_restore: StoredBackup,
    pub diff: Vec<BackupDifference>,
    pub plan: ApprovedRestorePlan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreExecutionSummary {
    pub attempted_steps: usize,
    pub verified_steps: usize,
    pub terminal_outcome: Option<DeviceWriteOutcome>,
}

#[derive(Clone, Debug)]
pub enum BackupAction {
    RefreshCatalog,
    CatalogRefreshed(Result<Vec<PathBuf>, String>),
    Capture,
    Captured(Result<StoredBackup, String>),
    SelectSource(PathBuf),
    SourceLoaded {
        path: PathBuf,
        result: Result<BackupSnapshot, String>,
    },
    ClearSource,
    PrepareRestore,
    RestorePrepared(Box<Result<PreparedRestoreBundle, String>>),
    ConfirmRestore {
        operator_text: String,
    },
    RestoreCompleted(Result<RestoreExecutionSummary, String>),
}

#[derive(Clone, Debug)]
pub enum BackupEffect {
    RefreshCatalog,
    Capture {
        context: BackupCaptureContext,
    },
    LoadSource {
        path: PathBuf,
    },
    PrepareRestore {
        source: Box<BackupSnapshot>,
        context: BackupCaptureContext,
    },
    ExecuteRestore {
        plan: ApprovedRestorePlan,
        confirmation: RestoreConfirmation,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BackupRestoreState {
    pub catalog: Vec<PathBuf>,
    pub last_capture: Option<StoredBackup>,
    pub source: Option<StoredBackup>,
    pub pre_restore: Option<StoredBackup>,
    pub diff: Vec<BackupDifference>,
    pub prepared_plan: Option<ApprovedRestorePlan>,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl BackupRestoreState {
    pub(crate) fn invalidate_prepared_operation(&mut self) {
        self.pre_restore = None;
        self.diff.clear();
        self.prepared_plan = None;
    }

    pub(crate) fn view(&self) -> BackupRestoreView {
        BackupRestoreView {
            catalog: self.catalog.clone(),
            last_capture: self.last_capture.as_ref().map(backup_summary),
            source: self.source.as_ref().map(backup_summary),
            pre_restore: self.pre_restore.as_ref().map(backup_summary),
            diff: self
                .diff
                .iter()
                .map(|entry| BackupDiffEntryView {
                    parameter_id: entry.parameter_id.as_str().to_owned(),
                    status: entry.status,
                })
                .collect(),
            prepared_plan: self.prepared_plan.as_ref().map(|plan| RestorePlanView {
                plan_hash: plan.plan_hash().to_owned(),
                challenge: plan.operator_confirmation_text(),
                steps: plan
                    .steps()
                    .iter()
                    .map(|step| RestoreStepView {
                        index: step.index(),
                        parameter_id: step.parameter_id().as_str().to_owned(),
                        expected_old: format!("{:?}", step.expected_old_raw().as_slice()),
                        target: format!("{:?}", step.target_raw().as_slice()),
                    })
                    .collect(),
                skipped: plan.skipped().len(),
            }),
            status: self.status.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupSummaryView {
    pub path: PathBuf,
    pub backup_id: u128,
    pub complete: bool,
    pub profile_id: String,
    pub profile_hash: String,
    pub values: usize,
    pub errors: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupDiffEntryView {
    pub parameter_id: String,
    pub status: BackupDiffStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreStepView {
    pub index: usize,
    pub parameter_id: String,
    pub expected_old: String,
    pub target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlanView {
    pub plan_hash: String,
    pub challenge: String,
    pub steps: Vec<RestoreStepView>,
    pub skipped: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackupRestoreView {
    pub catalog: Vec<PathBuf>,
    pub last_capture: Option<BackupSummaryView>,
    pub source: Option<BackupSummaryView>,
    pub pre_restore: Option<BackupSummaryView>,
    pub diff: Vec<BackupDiffEntryView>,
    pub prepared_plan: Option<RestorePlanView>,
    pub status: Option<String>,
    pub error: Option<String>,
}

fn backup_summary(stored: &StoredBackup) -> BackupSummaryView {
    BackupSummaryView {
        path: stored.path.clone(),
        backup_id: stored.snapshot.backup_id.get(),
        complete: stored.snapshot.is_complete(),
        profile_id: stored.snapshot.profile_id.as_str().to_owned(),
        profile_hash: stored.snapshot.profile_hash.clone(),
        values: stored.snapshot.values.len(),
        errors: stored.snapshot.errors.len(),
    }
}
