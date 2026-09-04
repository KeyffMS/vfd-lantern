use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(160)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let path = "crates/lantern-storage/src/audit.rs";
    replace_once(
        path,
        r#"struct PreparedBinding {
    plan_id: u128,
    request_id: u64,
    context_hash: String,
}
"#,
        r#"struct PreparedBinding {
    plan_id: u128,
    request_id: u64,
    session_id: u128,
    context_hash: String,
}
"#,
    );
    replace_once(
        path,
        r#"                let binding = PreparedBinding {
                    plan_id: preparation.plan_id.get(),
                    request_id: preparation.request_id.get(),
                    context_hash: preparation.context_hash.clone(),
                };
"#,
        r#"                let binding = PreparedBinding {
                    plan_id: preparation.plan_id.get(),
                    request_id: preparation.request_id.get(),
                    session_id: preparation.session_id.get(),
                    context_hash: preparation.context_hash.clone(),
                };
"#,
    );
    replace_once(
        path,
        r#"                let session_id = session_for_prepared_record(&self.root, token.token_id())
                    .unwrap_or(SessionId::new(0));
                if session_id.get() == 0 {
                    Err(AuditStorageError::InvalidToken)
                } else {
                    self.append(
                        session_id,
                        0,
                        "device_write_finalized",
                        finalize_body(token.token_id(), outcome, &read_back),
                    )
                }
"#,
        r#"                self.append(
                    SessionId::new(binding.session_id),
                    system_time_nanos(),
                    "device_write_finalized",
                    finalize_body(token.token_id(), outcome, &read_back),
                )
"#,
    );
    replace_once(
        path,
        r#"fn session_for_prepared_record(root: &Path, token_id: u128) -> Option<SessionId> {
    let needle = format!("\"token_id\":\"{token_id}\"");
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_str()?;
        let Some(session) = name
            .strip_prefix("audit_")
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|value| value.parse::<u128>().ok())
        else {
            continue;
        };
        if fs::read_to_string(entry.path())
            .ok()
            .is_some_and(|text| text.contains(&needle))
        {
            return Some(SessionId::new(session));
        }
    }
    None
}

"#,
        r#"fn system_time_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

"#,
    );
    replace_once(
        path,
        r#"        .append(true)
        .write(true)
        .mode(PRIVATE_FILE_MODE)
"#,
        r#"        .append(true)
        .mode(PRIVATE_FILE_MODE)
"#,
    );
    replace_once(
        path,
        r#"        AuditHead, AuditVerification, FilesystemAuditPort, head_path, journal_path, read_head,
        verify_audit_session, write_head,
"#,
        r#"        AuditVerification, FilesystemAuditPort, head_path, journal_path,
        verify_audit_session,
"#,
    );
    replace_once(
        path,
        r#"        let current_head: AuditHead = read_head(&head_path(directory.path(), session)).expect("read").expect("head");
        let mut journal = fs::read(journal_path(directory.path(), session)).expect("journal");
"#,
        r#"        let mut journal = fs::read(journal_path(directory.path(), session)).expect("journal");
"#,
    );
    replace_once(path, "        let _ = current_head;\n", "");
}
