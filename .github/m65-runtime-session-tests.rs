    fn writable_snapshot(revision: u64) -> lantern_app::WriteSessionSnapshot {
        let mut snapshot = super::unavailable_snapshot();
        snapshot.connected = true;
        snapshot.armed = true;
        snapshot.audit_healthy = true;
        snapshot.operation_idle = true;
        snapshot.guard_revision = revision;
        snapshot
    }

    fn cached_projection(runtime: &ProductionWriteRuntime) -> lantern_app::WriteSessionSnapshot {
        super::lock_projection(&runtime.session.projection).clone()
    }

    #[test]
    fn production_session_projection_changes_only_via_application_sync() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let idle = writable_snapshot(10);
        runtime.session.sync(idle.clone());

        runtime
            .session
            .begin_single_write(lantern_app::OperationId::new(7), lantern_app::PlanId::new(9))
            .expect("begin single write");
        assert_eq!(cached_projection(&runtime), idle);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(runtime.session.snapshot().armed);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::WriteConfirmed { .. }))
        ));

        let mut active = idle.clone();
        active.operation_idle = false;
        active.guard_revision = 11;
        runtime.session.sync(active.clone());
        assert_eq!(cached_projection(&runtime), active);

        runtime.session.finish_single_write(lantern_app::WriteOutcome::Executed(
            lantern_app::DeviceWriteOutcome::Verified,
        ));
        assert_eq!(cached_projection(&runtime), active);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::WriteFinished { .. }))
        ));

        let mut finished = idle;
        finished.guard_revision = 12;
        runtime.session.sync(finished.clone());
        assert_eq!(cached_projection(&runtime), finished);
        assert!(runtime.session.snapshot().operation_idle);
    }

    #[test]
    fn production_session_adapter_implements_restore_sequence_without_owning_session_state() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let idle = writable_snapshot(20);
        runtime.session.sync(idle.clone());
        let operation = lantern_app::OperationId::new(7);

        runtime
            .session
            .begin_restore(operation, "plan")
            .expect("begin restore");
        assert_eq!(cached_projection(&runtime), idle);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(runtime.session.restore_matches(operation, "plan", 0));
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::RestoreStarted { .. }
            ))
        ));

        let mut active = idle.clone();
        active.operation_idle = false;
        active.guard_revision = 21;
        runtime.session.sync(active.clone());
        runtime
            .session
            .advance_restore(operation, "plan", 1)
            .expect("advance restore");
        assert_eq!(cached_projection(&runtime), active);
        assert!(runtime.session.restore_matches(operation, "plan", 1));
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::RestoreAdvanced {
                next_index: 1
            }))
        ));

        runtime
            .session
            .finish_restore(operation, "plan")
            .expect("finish restore");
        assert_eq!(cached_projection(&runtime), active);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::RestoreFinished))
        ));

        let mut finished = idle;
        finished.guard_revision = 22;
        runtime.session.sync(finished.clone());
        assert_eq!(cached_projection(&runtime), finished);
        assert!(runtime.session.snapshot().operation_idle);
    }

    #[test]
    fn restrictive_barriers_never_grant_write_capability_before_authoritative_sync() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let armed = writable_snapshot(30);
        runtime.session.sync(armed.clone());

        runtime.session.disarm();
        assert_eq!(cached_projection(&runtime), armed);
        assert!(!runtime.session.snapshot().armed);
        assert!(runtime
            .session
            .begin_single_write(lantern_app::OperationId::new(1), lantern_app::PlanId::new(1))
            .is_err());
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::DisarmWrites))
        ));

        let mut disarmed = armed.clone();
        disarmed.armed = false;
        disarmed.guard_revision = 31;
        runtime.session.sync(disarmed.clone());
        assert_eq!(cached_projection(&runtime), disarmed);
        assert!(!runtime.session.snapshot().armed);

        let mut rearmed = armed;
        rearmed.guard_revision = 32;
        runtime.session.sync(rearmed.clone());
        runtime.session.degrade_audit_and_disarm();
        assert_eq!(cached_projection(&runtime), rearmed);
        let effective = runtime.session.snapshot();
        assert!(!effective.armed);
        assert!(!effective.audit_healthy);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::AuditPersistenceFailed { .. }
            ))
        ));

        let mut degraded = rearmed;
        degraded.armed = false;
        degraded.audit_healthy = false;
        degraded.guard_revision = 33;
        runtime.session.sync(degraded.clone());
        assert_eq!(cached_projection(&runtime), degraded);
        let effective = runtime.session.snapshot();
        assert!(!effective.armed);
        assert!(!effective.audit_healthy);
    }
}
