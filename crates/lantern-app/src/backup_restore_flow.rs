use std::path::PathBuf;

use lantern_domain::{
    BackupDiffStatus, BackupDifference, BackupSnapshot, DriveState, RestoreEligibility,
};

use crate::{ApprovedRestorePlan, RestoreConfirmation, WriteSessionSnapshot};

#[derive(Clone, Debug)]
pub struct BackupCaptureRequest {
    pub snapshot: WriteSessionSnapshot,
    pub profile_origin: String,
    pub adapter: String,
    pub link_settings: String,
    pub drive_state: DriveState,
}

#[derive(Clone, Debug)]
pub struct BackupCaptureResult {
    pub snapshot: BackupSnapshot,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PreparedRestoreResult {
    pub current: BackupSnapshot,
    pub current_path: PathBuf,
    pub diff: Vec<BackupDifference>,
    pub plan: ApprovedRestorePlan,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackupDiffSummary {
    pub unchanged: usize,
    pub changed: usize,
    pub only_source: usize,
    pub only_device: usize,
    pub unreadable: usize,
    pub incompatible: usize,
    pub not_restorable: usize,
}

impl BackupDiffSummary {
    #[must_use]
    pub fn from_diff(diff: &[BackupDifference]) -> Self {
        let mut summary = Self::default();
        for entry in diff {
            match entry.status {
                BackupDiffStatus::Unchanged => summary.unchanged += 1,
                BackupDiffStatus::Changed => summary.changed += 1,
                BackupDiffStatus::OnlyLeft => summary.only_source += 1,
                BackupDiffStatus::OnlyRight => summary.only_device += 1,
                BackupDiffStatus::Unreadable => summary.unreadable += 1,
                BackupDiffStatus::Incompatible => summary.incompatible += 1,
                BackupDiffStatus::NotRestorable => summary.not_restorable += 1,
            }
        }
        summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreStepView {
    pub index: usize,
    pub parameter_id: String,
    pub expected_old_raw: Vec<u16>,
    pub target_raw: Vec<u16>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackupRestoreView {
    pub source_path: Option<String>,
    pub source_backup_id: Option<u128>,
    pub source_complete: bool,
    pub last_capture_path: Option<String>,
    pub current_backup_id: Option<u128>,
    pub current_complete: bool,
    pub diff: Option<BackupDiffSummary>,
    pub prepared_plan_hash: Option<String>,
    pub prepared_confirmation: Option<String>,
    pub prepared_steps: Vec<RestoreStepView>,
    pub prepared_skipped: usize,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BackupRestoreState {
    source_path: Option<PathBuf>,
    source: Option<BackupSnapshot>,
    current: Option<BackupSnapshot>,
    last_capture_path: Option<PathBuf>,
    diff: Vec<BackupDifference>,
    prepared: Option<ApprovedRestorePlan>,
    status: Option<String>,
    error: Option<String>,
}

impl BackupRestoreState {
    #[must_use]
    pub(crate) fn view(&self) -> BackupRestoreView {
        BackupRestoreView {
            source_path: self
                .source_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            source_backup_id: self.source.as_ref().map(|backup| backup.backup_id.get()),
            source_complete: self
                .source
                .as_ref()
                .is_some_and(BackupSnapshot::is_complete),
            last_capture_path: self
                .last_capture_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            current_backup_id: self.current.as_ref().map(|backup| backup.backup_id.get()),
            current_complete: self
                .current
                .as_ref()
                .is_some_and(BackupSnapshot::is_complete),
            diff: (!self.diff.is_empty()).then(|| BackupDiffSummary::from_diff(&self.diff)),
            prepared_plan_hash: self
                .prepared
                .as_ref()
                .map(|plan| plan.plan_hash().to_owned()),
            prepared_confirmation: self
                .prepared
                .as_ref()
                .map(ApprovedRestorePlan::operator_confirmation_text),
            prepared_steps: self
                .prepared
                .as_ref()
                .map(|plan| {
                    plan.steps()
                        .iter()
                        .map(|step| RestoreStepView {
                            index: step.index(),
                            parameter_id: step.parameter_id().as_str().to_owned(),
                            expected_old_raw: step.expected_old_raw().as_slice().to_vec(),
                            target_raw: step.target_raw().as_slice().to_vec(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            prepared_skipped: self
                .prepared
                .as_ref()
                .map_or(0, |plan| plan.skipped().len()),
            status: self.status.clone(),
            error: self.error.clone(),
        }
    }

    pub(crate) fn clear_session(&mut self) {
        self.current = None;
        self.diff.clear();
        self.prepared = None;
        self.status = None;
        self.error = None;
    }

    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        self.status = None;
        self.error = Some(message.into());
    }

    pub(crate) fn begin_capture(&mut self) {
        self.status = Some("capturing complete backup through guarded read path".to_owned());
        self.error = None;
    }

    pub(crate) fn capture_finished(&mut self, result: Result<BackupCaptureResult, String>) {
        match result {
            Ok(result) => {
                let complete = result.snapshot.is_complete();
                let id = result.snapshot.backup_id.get();
                self.current = Some(result.snapshot);
                self.last_capture_path = Some(result.path);
                self.diff.clear();
                self.prepared = None;
                self.status = Some(format!("backup {id} captured; complete={complete}"));
                self.error = None;
            }
            Err(error) => self.fail(error),
        }
    }

    pub(crate) fn begin_load(&mut self, path: PathBuf) {
        self.source_path = Some(path);
        self.status = Some("loading and validating source backup".to_owned());
        self.error = None;
        self.diff.clear();
        self.prepared = None;
    }

    pub(crate) fn source_loaded(&mut self, result: Result<BackupSnapshot, String>) {
        match result {
            Ok(backup) => {
                let id = backup.backup_id.get();
                let complete = backup.is_complete();
                self.source = Some(backup);
                self.status = Some(format!("source backup {id} loaded; complete={complete}"));
                self.error = None;
            }
            Err(error) => {
                self.source = None;
                self.fail(error);
            }
        }
    }

    pub(crate) fn source(&self) -> Option<&BackupSnapshot> {
        self.source.as_ref()
    }

    pub(crate) fn prepared(&self) -> Option<&ApprovedRestorePlan> {
        self.prepared.as_ref()
    }

    pub(crate) fn begin_prepare_restore(&mut self) {
        self.status = Some(
            "capturing fresh pre-restore backup and preparing guarded restore plan".to_owned(),
        );
        self.error = None;
        self.prepared = None;
        self.diff.clear();
    }

    pub(crate) fn restore_prepared(&mut self, result: Result<PreparedRestoreResult, String>) {
        match result {
            Ok(result) => {
                let steps = result.plan.steps().len();
                let skipped = result.plan.skipped().len();
                self.current = Some(result.current);
                self.last_capture_path = Some(result.current_path);
                self.diff = result.diff;
                self.prepared = Some(result.plan);
                self.status = Some(format!(
                    "restore plan prepared; steps={steps} skipped={skipped}; exact confirmation required"
                ));
                self.error = None;
            }
            Err(error) => self.fail(error),
        }
    }

    pub(crate) fn take_prepared(&mut self) -> Option<ApprovedRestorePlan> {
        self.prepared.take()
    }

    pub(crate) fn confirmation_mismatch(&mut self) {
        self.error = Some("operator confirmation does not exactly match restore plan".to_owned());
    }

    pub(crate) fn begin_execute(&mut self) {
        self.status = Some("guarded restore execution started".to_owned());
        self.error = None;
    }

    pub(crate) fn restore_finished(&mut self, result: Result<RestoreExecutionSummary, String>) {
        self.prepared = None;
        match result {
            Ok(summary) => {
                self.status = Some(format!(
                    "restore completed; verified_steps={}",
                    summary.verified_steps
                ));
                self.error = None;
            }
            Err(error) => self.fail(error),
        }
    }

    pub(crate) fn clear_prepared(&mut self) {
        self.prepared = None;
        self.diff.clear();
        self.status = Some("prepared restore cleared; no write executed".to_owned());
        self.error = None;
    }
}

#[derive(Clone, Debug)]
pub enum BackupRestoreAction {
    Capture,
    CaptureFinished(Result<BackupCaptureResult, String>),
    LoadSource(PathBuf),
    SourceLoaded(Result<BackupSnapshot, String>),
    PrepareRestore,
    RestorePrepared(Box<Result<PreparedRestoreResult, String>>),
    ConfirmRestore { operator_text: String },
    ClearPrepared,
    RestoreFinished(Result<RestoreExecutionSummary, String>),
}

#[derive(Clone, Debug)]
pub enum BackupRestoreEffect {
    Capture {
        request: Box<BackupCaptureRequest>,
    },
    LoadSource {
        path: PathBuf,
    },
    PrepareRestore {
        source: Box<BackupSnapshot>,
        request: Box<BackupCaptureRequest>,
    },
    ExecuteRestore {
        plan: Box<ApprovedRestorePlan>,
        confirmation: RestoreConfirmation,
        snapshot: WriteSessionSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoreExecutionSummary {
    pub verified_steps: usize,
}

#[must_use]
pub fn restorable_change_count(diff: &[BackupDifference]) -> usize {
    diff.iter()
        .filter(|entry| {
            entry.status == BackupDiffStatus::Changed
                && entry.eligibility == RestoreEligibility::Eligible
        })
        .count()
}

#[cfg(test)]
mod tests {
    use lantern_domain::{BackupDiffStatus, BackupDifference, ParameterId, RestoreEligibility};

    use super::{BackupDiffSummary, restorable_change_count};

    #[test]
    fn diff_summary_and_restore_count_use_one_semantic_model() {
        let changed = BackupDifference {
            parameter_id: ParameterId::parse("p.changed").expect("id"),
            status: BackupDiffStatus::Changed,
            eligibility: RestoreEligibility::Eligible,
            left: None,
            right: None,
        };
        let blocked = BackupDifference {
            parameter_id: ParameterId::parse("p.blocked").expect("id"),
            status: BackupDiffStatus::NotRestorable,
            eligibility: RestoreEligibility::Dangerous,
            left: None,
            right: None,
        };
        let diff = vec![changed, blocked];
        let summary = BackupDiffSummary::from_diff(&diff);
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.not_restorable, 1);
        assert_eq!(restorable_change_count(&diff), 1);
    }
}
