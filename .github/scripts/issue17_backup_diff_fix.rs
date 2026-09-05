use std::fs;

fn replace_once(text: &mut String, old: &str, new: &str) {
    if text.contains(new) {
        return;
    }
    let Some(index) = text.find(old) else {
        panic!("anchor not found:\n{old}");
    };
    text.replace_range(index..index + old.len(), new);
}

fn main() {
    let path = "crates/lantern-app/src/backup.rs";
    let mut text = fs::read_to_string(path).expect("read backup.rs");

    replace_once(
        &mut text,
        ".map_or(RestoreEligibility::MissingWriteFunction, restore_eligibility);",
        ".map_or(RestoreEligibility::Eligible, restore_eligibility);",
    );

    replace_once(
        &mut text,
        r#"        let diff = semantic_backup_diff(&left, &right, None);
        assert_eq!(diff[0].status, BackupDiffStatus::NotRestorable);
        assert_eq!(diff[0].eligibility, RestoreEligibility::MissingWriteFunction);
        assert_eq!(diff[1].status, BackupDiffStatus::NotRestorable);
        assert_eq!(diff[2].status, BackupDiffStatus::OnlyRight);

        let incompatible = snapshot(&"bb".repeat(32), right.values.clone());
        assert!(semantic_backup_diff(&left, &incompatible, None)
            .iter()
            .filter(|entry| entry.left.is_some() && entry.right.is_some())
            .all(|entry| entry.status == BackupDiffStatus::Incompatible));"#,
        r#"        let diff = semantic_backup_diff(&left, &right, None);
        let by_id = |id: &ParameterId| {
            diff.iter()
                .find(|entry| &entry.parameter_id == id)
                .expect("diff entry")
        };
        assert_eq!(by_id(&id1).status, BackupDiffStatus::Unchanged);
        assert_eq!(by_id(&id1).eligibility, RestoreEligibility::Eligible);
        assert_eq!(by_id(&id2).status, BackupDiffStatus::Changed);
        let id3 = ParameterId::parse("p.three").expect("id");
        assert_eq!(by_id(&id3).status, BackupDiffStatus::OnlyRight);

        let incompatible = snapshot(&"bb".repeat(32), right.values.clone());
        assert!(semantic_backup_diff(&left, &incompatible, None)
            .iter()
            .filter(|entry| entry.left.is_some() && entry.right.is_some())
            .all(|entry| entry.status == BackupDiffStatus::Incompatible));"#,
    );

    fs::write(path, text).expect("write backup.rs");
}
