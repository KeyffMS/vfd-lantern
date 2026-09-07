mod restore_tests {
    use std::collections::BTreeMap;

    use lantern_domain::{
        BackupCompleteness, BackupId, BackupParameterValue, BackupSnapshot, OperationAuditOutcome,
        TelemetryQuality, UtcTimestamp,
    };

    use super::*;
    use crate::{ApprovedRestorePlan, RestoreConfirmation, RestoreOperationPermit};

    fn backup(profile: &ValidatedDeviceProfile, id: u128, value: u16) -> BackupSnapshot {
        let parameter_id = ParameterId::parse("config.acceleration").unwrap();
        let parameter = profile.parameter(&parameter_id).unwrap();
        let registers = raw(value);
        let value = BackupParameterValue {
            code: parameter.code().to_owned(),
            engineering: parameter.codec().decode(registers.as_slice()).unwrap(),
            raw: registers,
            quantity: "time".to_owned(),
            unit: "s".to_owned(),
            quality: TelemetryQuality::Good,
            observed_at: MonotonicInstant::from_nanos(1),
            access: parameter.access(),
            restore_policy: parameter.restore_policy(),
        };
        BackupSnapshot {
            app_version: "test".to_owned(),
            build_id: "restore-contract".to_owned(),
            backup_id: BackupId::new(id),
            started_at: UtcTimestamp::from_unix_nanos(1),
            finished_at: UtcTimestamp::from_unix_nanos(2),
            profile_id: profile.profile_id().clone(),
            profile_revision: profile.revision(),
            profile_origin: "Packaged".to_owned(),
            source_hash: profile.source_hash().to_hex(),
            profile_hash: profile.profile_hash().to_hex(),
            device_fingerprint: base_snapshot(profile).fingerprint,
            vendor: profile.vendor().to_owned(),
            model: profile.model().to_owned(),
            slave_id: 1,
            adapter: "mock".to_owned(),
            link_settings: "9600-8N1".to_owned(),
            drive_state: DriveState::Stopped,
            completeness: BackupCompleteness::Complete,
            values: BTreeMap::from([(parameter_id, value)]),
            errors: Box::new([]),
        }
    }

    async fn prepared(
        following_reads: Vec<RawRegisters>,
    ) -> (WriteCoordinator, Arc<Trace>, Arc<RecordingSession>, ApprovedRestorePlan) {
        let profile = test_profile();
        let mut reads = vec![raw(0), raw(90)];
        reads.extend(following_reads);
        let (mut coordinator, trace, session) = runtime(
            Arc::clone(&profile),
            base_snapshot(&profile),
            reads,
            RuntimeOptions { read_back_attempts: 1, ..RuntimeOptions::default() },
        );
        let plan = coordinator.prepare_restore_plan(
            &backup(&profile, 1, 100), &backup(&profile, 2, 90),
        ).await.unwrap();
        assert_eq!(plan.steps().len(), 1);
        assert_eq!(plan.steps()[0].expected_old_raw(), &raw(90));
        assert_eq!(plan.steps()[0].target_raw(), &raw(100));
        assert!(trace.writes.lock().unwrap().is_empty());
        assert!(trace.operation_starts.lock().unwrap().is_empty());
        (coordinator, trace, session, plan)
    }

    async fn begin(coordinator: &mut WriteCoordinator, plan: ApprovedRestorePlan) -> RestoreOperationPermit {
        let challenge = plan.operator_confirmation_text();
        coordinator.begin_restore(plan, RestoreConfirmation::Confirm { challenge }).await.unwrap()
    }

    #[tokio::test]
    async fn restore_orders_durable_audit_single_write_verification_and_finalization() {
        let (mut coordinator, trace, session, plan) = prepared(
            vec![raw(0), raw(90), raw(0), raw(90), raw(100)],
        ).await;
        let mut permit = begin(&mut coordinator, plan).await;
        assert!(permit.is_active());
        assert_eq!(permit.next_index(), 0);
        assert!(permit.results().is_empty());
        assert!(!session.snapshot().operation_idle);
        assert!(matches!(coordinator.execute_restore_step(&mut permit, 1).await,
            Err(WriteCoordinatorError::InvalidRestorePermit)));
        assert!(trace.writes.lock().unwrap().is_empty());
        assert_eq!(coordinator.execute_restore_step(&mut permit, 0).await.unwrap(), DeviceWriteOutcome::Verified);
        assert_eq!(permit.next_index(), 1);
        assert_eq!(permit.results()[0].outcome, DeviceWriteOutcome::Verified);
        assert!(matches!(coordinator.execute_restore_step(&mut permit, 0).await,
            Err(WriteCoordinatorError::InvalidRestorePermit)));
        coordinator.finish_restore(permit).await.unwrap();
        assert_eq!(trace.writes.lock().unwrap().as_slice(), &[raw(100)]);
        assert_eq!(trace.operation_finishes.lock().unwrap()[0].outcome, OperationAuditOutcome::Completed);
        assert_eq!(trace.events.lock().unwrap().as_slice(), &[
            "read", "read", "read", "read", "operation:begin", "read", "read",
            "audit:prepare", "write", "read", "audit:finalize", "operation:finish", "session:disarm",
        ]);
        assert!(session.snapshot().operation_idle);
        assert!(!session.snapshot().armed);
    }

    #[tokio::test]
    async fn wrong_confirmation_disarms_without_audit_start_or_write() {
        let (mut coordinator, trace, session, plan) = prepared(vec![]).await;
        let result = coordinator.begin_restore(plan, RestoreConfirmation::Confirm {
            challenge: "wrong".to_owned(),
        }).await;
        assert!(matches!(result, Err(WriteCoordinatorError::RestoreRejected)));
        assert!(trace.operation_starts.lock().unwrap().is_empty());
        assert!(trace.writes.lock().unwrap().is_empty());
        assert!(!session.snapshot().armed);
    }

    #[tokio::test]
    async fn explicit_abort_finishes_audit_without_writing() {
        let (mut coordinator, trace, session, plan) = prepared(vec![raw(0), raw(90)]).await;
        let permit = begin(&mut coordinator, plan).await;
        coordinator.abort_restore(permit, "operator cancelled").await.unwrap();
        assert!(trace.writes.lock().unwrap().is_empty());
        assert_eq!(trace.operation_finishes.lock().unwrap()[0].outcome, OperationAuditOutcome::Aborted);
        assert!(session.snapshot().operation_idle);
        assert!(!session.snapshot().armed);
    }

    #[tokio::test]
    async fn changed_session_or_old_value_aborts_and_invalidates_permit() {
        for changed_session in [false, true] {
            let mut reads = vec![raw(0), raw(90)];
            if !changed_session { reads.extend([raw(0), raw(91)]); }
            let (mut coordinator, trace, session, plan) = prepared(reads).await;
            let mut permit = begin(&mut coordinator, plan).await;
            if changed_session { session.snapshot.lock().unwrap().connected = false; }
            assert!(matches!(coordinator.execute_restore_step(&mut permit, 0).await,
                Err(WriteCoordinatorError::InvalidRestorePermit)));
            assert!(!permit.is_active());
            assert!(matches!(coordinator.execute_restore_step(&mut permit, 0).await,
                Err(WriteCoordinatorError::InvalidRestorePermit)));
            assert!(trace.writes.lock().unwrap().is_empty());
            assert_eq!(trace.operation_finishes.lock().unwrap().len(), 1);
            assert_eq!(trace.operation_finishes.lock().unwrap()[0].outcome, OperationAuditOutcome::Aborted);
            assert!(!session.snapshot().armed);
        }
    }

    #[tokio::test]
    async fn readback_mismatch_terminates_restore_after_exactly_one_write() {
        let (mut coordinator, trace, session, plan) = prepared(
            vec![raw(0), raw(90), raw(0), raw(90), raw(99)],
        ).await;
        let mut permit = begin(&mut coordinator, plan).await;
        let outcome = coordinator.execute_restore_step(&mut permit, 0).await.unwrap();
        assert_eq!(outcome, DeviceWriteOutcome::ReadBackMismatch);
        assert!(!permit.is_active());
        assert_eq!(permit.results()[0].outcome, outcome);
        assert_eq!(trace.writes.lock().unwrap().as_slice(), &[raw(100)]);
        assert_eq!(trace.operation_finishes.lock().unwrap()[0].outcome, OperationAuditOutcome::Aborted);
        assert!(!session.snapshot().armed);
        assert!(matches!(coordinator.finish_restore(permit).await,
            Err(WriteCoordinatorError::InvalidRestorePermit)));
    }

    #[tokio::test]
    async fn changed_preconditions_and_unavailable_audit_prevent_restore_start() {
        for fault in 0..4 {
            let profile = test_profile();
            let (mut coordinator, trace, _) = runtime(
                Arc::clone(&profile), base_snapshot(&profile), vec![raw(0), raw(91)],
                RuntimeOptions {
                    process_writes_enabled: fault != 0,
                    trusted: fault != 1,
                    ..RuntimeOptions::default()
                },
            );
            let mut source = backup(&profile, 1, 100);
            if fault == 2 { source.completeness = BackupCompleteness::Incomplete; }
            assert!(coordinator.prepare_restore_plan(&source, &backup(&profile, 2, 90)).await.is_err());
            assert!(trace.writes.lock().unwrap().is_empty());
            assert!(trace.operation_starts.lock().unwrap().is_empty());
        }
        let (mut coordinator, trace, session, plan) = prepared(vec![raw(0), raw(90)]).await;
        coordinator.audit = Arc::new(RecordingAudit {
            trace: Arc::clone(&trace), available: false, fail_decision: false, fail_prepare: false,
        });
        let challenge = plan.operator_confirmation_text();
        assert!(matches!(coordinator.begin_restore(plan, RestoreConfirmation::Confirm { challenge }).await,
            Err(WriteCoordinatorError::RestoreAuditUnavailable)));
        assert!(trace.writes.lock().unwrap().is_empty());
        assert!(!session.snapshot().audit_healthy);
        assert!(!session.snapshot().armed);
    }

    struct CaptureBus {
        mode: u8,
        session: Arc<RecordingSession>,
    }

    impl ReadBusPort for CaptureBus {
        fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            assert_eq!(request.function(), lantern_domain::ModbusFunction::ReadHoldingRegisters);
            self.session.trace.events.lock().unwrap().push("backup:read");
            if self.mode == 3 { self.session.snapshot.lock().unwrap().connected = false; }
            let result = match self.mode {
                1 => Ok(RawRegisters::new(vec![90, 91]).unwrap()),
                2 => Err(crate::BusError::ResponseTimeout),
                _ => Ok(raw(90)),
            };
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn backup_capture_preserves_read_failures_and_never_fabricates_complete_values() {
        for mode in 0..5 {
            let profile = test_profile();
            let (_, trace, session) = runtime(
                Arc::clone(&profile), base_snapshot(&profile), vec![], RuntimeOptions::default(),
            );
            if mode == 4 { session.snapshot.lock().unwrap().connected = false; }
            let mut capture = crate::BackupCoordinator::new(
                Arc::new(CaptureBus { mode, session: Arc::clone(&session) }),
                Arc::new(TestTrust { profile: Arc::clone(&profile), trusted: true }),
                Arc::new(TestClock::new(1)), session, Duration::from_secs(1),
            ).unwrap();
            let result = capture.capture(crate::BackupCaptureContext {
                app_version: "test".to_owned(), build_id: "capture".to_owned(),
                profile_origin: "LocalUntrusted".to_owned(), adapter: "mock".to_owned(),
                link_settings: "9600-8N1".to_owned(), drive_state: DriveState::Stopped,
                started_at: UtcTimestamp::from_unix_nanos(1),
                finished_at: UtcTimestamp::from_unix_nanos(2),
            }).await;
            if mode == 4 {
                assert!(matches!(result, Err(crate::BackupError::SessionUnavailable)));
                assert!(trace.events.lock().unwrap().is_empty());
                continue;
            }
            let snapshot = result.unwrap();
            assert_eq!(snapshot.profile_hash, profile.profile_hash().to_hex());
            assert_eq!(snapshot.profile_origin, "LocalUntrusted");
            assert_eq!(snapshot.is_complete(), mode == 0);
            if mode == 0 {
                assert!(snapshot.errors.is_empty());
                assert_eq!(snapshot.values.len(), 1);
                assert_eq!(snapshot.values.values().next().unwrap().raw, raw(90));
            } else {
                assert!(snapshot.values.is_empty());
                assert!(!snapshot.errors.is_empty());
            }
            assert!(trace.writes.lock().unwrap().is_empty());
        }
    }
}
