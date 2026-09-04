use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else { panic!("anchor not found in {}:\n{}", path.display(), old); };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    audit: Option<Arc<FilesystemAuditPort>>,\n    trust: Option<Arc<RuntimeProfileTrust>>,\n",
        "    audit: Option<Arc<dyn AuditPort>>,\n    trust: Option<Arc<dyn ProfileTrustPort>>,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        r#"        let audit = match FilesystemAuditPort::new(audit_directory) {
            Ok(port) => Some(Arc::new(port)),
            Err(error) => {
                eprintln!(
                    "durable audit unavailable; production writes remain fail-closed: {error}"
                );
                None
            }
        };
        let trust = Some(Arc::new(RuntimeProfileTrust::new(
            registry,
            trust_store_path,
        )));
        let session = Arc::new(RuntimeSessionControl::new(action_tx.clone()));
        Self {
            coordinator: Arc::new(AsyncMutex::new(None)),
            session,
            audit,
            trust,
            clock: Arc::new(RuntimeWriteClock::new()),
            config: WriteCoordinatorConfig {
                process_writes_enabled,
                ..WriteCoordinatorConfig::default()
            },
            action_tx,
        }
    }

    pub async fn attach_bus(&self, handle: BusActorHandle) {
"#,
        r#"        let audit: Option<Arc<dyn AuditPort>> = match FilesystemAuditPort::new(audit_directory) {
            Ok(port) => Some(Arc::new(port)),
            Err(error) => {
                eprintln!(
                    "durable audit unavailable; production writes remain fail-closed: {error}"
                );
                None
            }
        };
        let trust: Option<Arc<dyn ProfileTrustPort>> = Some(Arc::new(RuntimeProfileTrust::new(
            registry,
            trust_store_path,
        )));
        Self::from_adapters(action_tx, audit, trust, process_writes_enabled)
    }

    fn from_adapters(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        audit: Option<Arc<dyn AuditPort>>,
        trust: Option<Arc<dyn ProfileTrustPort>>,
        process_writes_enabled: bool,
    ) -> Self {
        let session = Arc::new(RuntimeSessionControl::new(action_tx.clone()));
        Self {
            coordinator: Arc::new(AsyncMutex::new(None)),
            session,
            audit,
            trust,
            clock: Arc::new(RuntimeWriteClock::new()),
            config: WriteCoordinatorConfig {
                process_writes_enabled,
                ..WriteCoordinatorConfig::default()
            },
            action_tx,
        }
    }

    pub async fn attach_bus(&self, handle: BusActorHandle) {
        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        self.attach_ports(read_bus, write_bus).await;
    }

    async fn attach_ports(
        &self,
        read_bus: Arc<dyn ReadBusPort>,
        write_bus: Arc<dyn WriteBusPort>,
    ) {
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        r#"        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        let audit: Arc<dyn AuditPort> = audit;
        let trust: Arc<dyn ProfileTrustPort> = trust;
"#,
        "",
    );

    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "struct RuntimeWriteClock {\n",
        r#"#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use lantern_app::{
        AuditPort, BusError, BusFuture, PreparedBusWrite, ProfileRegistry, ProfileTrustPort,
        RawRegisters, ReadBusPort, ReadBusRequest, WriteBusPort,
    };
    use lantern_storage::RuntimeProfileTrust;
    use tokio::sync::mpsc;

    use super::ProductionWriteRuntime;

    #[derive(Default)]
    struct CountingBus {
        writes: AtomicUsize,
    }

    impl ReadBusPort for CountingBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            Box::pin(async { Err(BusError::Shutdown) })
        }
    }

    impl WriteBusPort for CountingBus {
        fn execute(&self, _request: PreparedBusWrite) -> BusFuture<'static, ()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct AvailableAudit;

    impl AuditPort for AvailableAudit {
        fn is_available(&self) -> bool {
            true
        }
    }

    fn trust_adapter() -> Arc<dyn ProfileTrustPort> {
        Arc::new(RuntimeProfileTrust::new(
            Arc::new(ProfileRegistry::default()),
            "unused-test-trust.json".into(),
        ))
    }

    async fn attach_counting_bus(runtime: &ProductionWriteRuntime) -> Arc<CountingBus> {
        let bus = Arc::new(CountingBus::default());
        let read: Arc<dyn ReadBusPort> = bus.clone();
        let write: Arc<dyn WriteBusPort> = bus.clone();
        runtime.attach_ports(read, write).await;
        bus
    }

    #[tokio::test]
    async fn missing_audit_adapter_never_mints_write_capability_or_touches_bus() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_none());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_profile_trust_adapter_never_mints_write_capability_or_touches_bus() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let audit: Arc<dyn AuditPort> = Arc::new(AvailableAudit);
        let runtime = ProductionWriteRuntime::from_adapters(tx, Some(audit), None, true);
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_none());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn both_required_adapters_mint_coordinator_without_implicit_write() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let audit: Arc<dyn AuditPort> = Arc::new(AvailableAudit);
        let runtime = ProductionWriteRuntime::from_adapters(
            tx,
            Some(audit),
            Some(trust_adapter()),
            true,
        );
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_some());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }
}

struct RuntimeWriteClock {
"#,
    );

    replace_once(
        "scripts/check-architecture.sh",
        "printf 'architecture checks passed\\n'\n",
        r#"if ! grep -q 'missing_audit_adapter_never_mints_write_capability_or_touches_bus' crates/vfd-lantern/src/write_runtime.rs \
    || ! grep -q 'missing_profile_trust_adapter_never_mints_write_capability_or_touches_bus' crates/vfd-lantern/src/write_runtime.rs; then
    printf 'issue #23 requires fail-closed composition tests for missing audit/trust adapters\n' >&2
    exit 1
fi

if grep -q 'operator_text\.trim()' crates/lantern-app/src/application.rs; then
    printf 'phase-2 operator confirmation must be exact; whitespace normalization is forbidden\n' >&2
    exit 1
fi

printf 'architecture checks passed\n'
"#,
    );
}
