use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let observability = "crates/lantern-storage/src/observability.rs";
    replace_once(
        observability,
        r#"    use std::{fs, io::Read};
"#,
        r#"    use std::{
        fs,
        io::{Read, Write},
        sync::mpsc,
        time::Duration,
    };
"#,
    );
    replace_once(
        observability,
        r#"    use tracing_subscriber::layer::SubscriberExt;
"#,
        r#"    use tracing_appender::non_blocking::NonBlockingBuilder;
    use tracing_subscriber::layer::SubscriberExt;
"#,
    );
    replace_once(
        observability,
        r#"    #[test]
    fn diagnostic_retention_is_exactly_seven_and_ignores_non_log_files() {
"#,
        r#"    struct FirstWriteBlocker {
        blocked_once: bool,
        started: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }

    impl Write for FirstWriteBlocker {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.blocked_once {
                self.blocked_once = true;
                let _ = self.started.send(());
                let _ = self.release.recv();
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn non_blocking_log_queue_pressure_is_lossy_and_counted() {
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let sink = FirstWriteBlocker {
            blocked_once: false,
            started: started_tx,
            release: release_rx,
        };
        let (mut writer, guard) = NonBlockingBuilder::default()
            .buffered_lines_limit(1)
            .lossy(true)
            .finish(sink);
        let counter = writer.error_counter();

        writer.write_all(b"first\n").expect("first line");
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker reached blocked first write");
        writer.write_all(b"queued\n").expect("queued line");
        for _ in 0..64 {
            let _ = writer.write_all(b"drop-candidate\n");
        }
        assert!(counter.dropped_lines() > 0, "queue pressure must be counted");
        release_tx.send(()).expect("release worker");
        drop(writer);
        drop(guard);
    }

    #[test]
    fn diagnostic_retention_is_exactly_seven_and_ignores_non_log_files() {
"#,
    );

    let audit = "crates/lantern-storage/src/audit.rs";
    replace_once(
        audit,
        r#"    use super::{
        AuditVerification, FilesystemAuditPort, head_path, journal_path, verify_audit_session,
    };
"#,
        r#"    use super::{
        AuditHead, AuditVerification, FilesystemAuditPort, head_path, journal_path, read_head,
        verify_audit_session, write_head,
    };
"#,
    );
    replace_once(
        audit,
        r#"    #[test]
    fn missing_head_is_never_treated_as_valid() {
"#,
        r#"    #[tokio::test]
    async fn prepare_and_operation_begin_return_no_capability_when_storage_is_unavailable() {
        let directory = tempdir().expect("tempdir");
        let audit_path = directory.path().join("audit");
        let audit = FilesystemAuditPort::new(&audit_path).expect("audit");
        fs::remove_dir(&audit_path).expect("remove empty audit directory");
        fs::write(&audit_path, b"not-a-directory").expect("replace root with file");

        assert!(
            audit
                .prepare_device_write(preparation(SessionId::new(40)))
                .await
                .is_err(),
            "failed durable prepare must return no PreparedToken"
        );
        assert!(
            audit
                .begin_operation(OperationAuditStart {
                    operation_id: OperationId::new(41),
                    backup_id: BackupId::new(42),
                    plan_hash: "restore-plan".into(),
                    session_id: SessionId::new(40),
                    fingerprint: fingerprint(),
                    profile_hash: "profile-hash".into(),
                    at: MonotonicInstant::from_nanos(20),
                })
                .await
                .is_err(),
            "failed durable operation start must return no OperationToken"
        );
    }

    #[tokio::test]
    async fn failed_device_finalize_consumes_prepared_token_binding() {
        let directory = tempdir().expect("tempdir");
        let audit_path = directory.path().join("audit");
        let displaced = directory.path().join("audit-displaced");
        let audit = FilesystemAuditPort::new(&audit_path).expect("audit");
        let preparation = preparation(SessionId::new(50));
        let token = audit
            .prepare_device_write(preparation.clone())
            .await
            .expect("prepare");
        let token_id = token.token_id();

        fs::rename(&audit_path, &displaced).expect("move durable audit directory");
        fs::write(&audit_path, b"not-a-directory").expect("block audit root");
        assert!(
            audit
                .finalize_device_write(
                    token,
                    DeviceWriteOutcome::Verified,
                    ReadBackEvidence::Verified {
                        attempts: 1,
                        raw: raw(100),
                    },
                )
                .await
                .is_err()
        );

        fs::remove_file(&audit_path).expect("remove blocker");
        fs::rename(&displaced, &audit_path).expect("restore audit directory");
        let forged_retry = PreparedToken::for_preparation(token_id, &preparation);
        assert!(
            audit
                .finalize_device_write(
                    forged_retry,
                    DeviceWriteOutcome::Verified,
                    ReadBackEvidence::NotAttempted,
                )
                .await
                .is_err(),
            "finalize failure must still consume the single-use token binding"
        );
    }

    #[tokio::test]
    async fn verifier_classifies_record_missing_head_mismatch_and_unsupported_schema() {
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new(directory.path()).expect("audit");

        let missing = SessionId::new(60);
        for plan in 1..=3 {
            audit
                .record_decision(DecisionAuditRecord {
                    plan_id: PlanId::new(plan),
                    session_id: missing,
                    fingerprint: fingerprint(),
                    profile_hash: "profile-hash".into(),
                    parameter_id: parameter(),
                    context_hash: None,
                    decision: DecisionOutcome::Cancelled,
                    at: MonotonicInstant::from_nanos(plan),
                })
                .await
                .expect("decision");
        }
        let journal = journal_path(directory.path(), missing);
        let text = fs::read_to_string(&journal).expect("journal");
        let lines = text.lines().collect::<Vec<_>>();
        fs::write(&journal, format!("{}\n{}\n", lines[0], lines[2]))
            .expect("remove middle record");
        assert_eq!(
            verify_audit_session(directory.path(), missing),
            AuditVerification::RecordMissing
        );

        let mismatch = SessionId::new(61);
        audit
            .prepare_device_write(preparation(mismatch))
            .await
            .expect("prepare mismatch");
        let mismatch_head_path = head_path(directory.path(), mismatch);
        let mut mismatch_head: AuditHead = read_head(&mismatch_head_path)
            .expect("read head")
            .expect("head");
        mismatch_head.head_hash = "00".repeat(32);
        write_head(&mismatch_head_path, &mismatch_head).expect("rewrite head");
        assert_eq!(
            verify_audit_session(directory.path(), mismatch),
            AuditVerification::HeadMismatch
        );

        let unsupported = SessionId::new(62);
        audit
            .prepare_device_write(preparation(unsupported))
            .await
            .expect("prepare unsupported");
        let unsupported_head_path = head_path(directory.path(), unsupported);
        let mut unsupported_head: AuditHead = read_head(&unsupported_head_path)
            .expect("read head")
            .expect("head");
        unsupported_head.schema_version = 999;
        write_head(&unsupported_head_path, &unsupported_head).expect("rewrite unsupported head");
        assert_eq!(
            verify_audit_session(directory.path(), unsupported),
            AuditVerification::UnsupportedSchema
        );
    }

    #[test]
    fn missing_head_is_never_treated_as_valid() {
"#,
    );

    let architecture = "scripts/check-architecture.sh";
    replace_once(
        architecture,
        r#"if grep -R -n -E '\b(WriteCoordinator|PreparedBusWrite)\b' crates/vfd-lantern/src; then
    printf 'production composition root exposes guarded writes before #22/#23\n' >&2
    exit 1
fi

cargo metadata --locked --no-deps --format-version 1 >/dev/null
"#,
        r#"if grep -R -n -E '\b(WriteCoordinator|PreparedBusWrite)\b' crates/vfd-lantern/src; then
    printf 'production composition root exposes guarded writes before #22/#23\n' >&2
    exit 1
fi

if grep -R -n -E '\b(TcpStream|TcpListener|UdpSocket|reqwest|hyper|ureq)\b' \
    crates/lantern-app/src crates/lantern-storage/src crates/vfd-lantern/src; then
    printf 'network endpoint/client path found in application, storage, or composition root\n' >&2
    exit 1
fi

cargo metadata --locked --no-deps --format-version 1 >/dev/null
"#,
    );
}
