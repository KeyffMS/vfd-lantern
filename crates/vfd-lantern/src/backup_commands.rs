use anyhow::{Context, Result};
use lantern_app::{BackupDiffStatus, semantic_backup_diff};
use lantern_storage::read_backup;

use crate::cli::BackupCommand;

pub fn run(command: BackupCommand) -> Result<()> {
    match command {
        BackupCommand::Inspect { file } => {
            let backup = read_backup(&file)
                .with_context(|| format!("failed to read backup {}", file.display()))?;
            println!(
                "backup={} complete={} profile={} revision={} origin={} values={} errors={} device={} slave={} drive_state={:?}",
                backup.backup_id.get(),
                backup.is_complete(),
                backup.profile_id.as_str(),
                backup.profile_revision,
                backup.profile_origin,
                backup.values.len(),
                backup.errors.len(),
                backup.device_fingerprint.as_str(),
                backup.slave_id,
                backup.drive_state,
            );
            Ok(())
        }
        BackupCommand::Diff { left, right } => {
            let left_backup = read_backup(&left)
                .with_context(|| format!("failed to read backup {}", left.display()))?;
            let right_backup = read_backup(&right)
                .with_context(|| format!("failed to read backup {}", right.display()))?;
            let diff = semantic_backup_diff(&left_backup, &right_backup, None);
            let mut counts = [0_usize; 7];
            for entry in &diff {
                counts[status_index(entry.status)] =
                    counts[status_index(entry.status)].saturating_add(1);
                println!("{}\t{:?}", entry.parameter_id.as_str(), entry.status);
            }
            println!(
                "summary unchanged={} changed={} only_left={} only_right={} unreadable={} incompatible={} not_restorable={}",
                counts[0], counts[1], counts[2], counts[3], counts[4], counts[5], counts[6]
            );
            Ok(())
        }
    }
}

const fn status_index(status: BackupDiffStatus) -> usize {
    match status {
        BackupDiffStatus::Unchanged => 0,
        BackupDiffStatus::Changed => 1,
        BackupDiffStatus::OnlyLeft => 2,
        BackupDiffStatus::OnlyRight => 3,
        BackupDiffStatus::Unreadable => 4,
        BackupDiffStatus::Incompatible => 5,
        BackupDiffStatus::NotRestorable => 6,
    }
}
