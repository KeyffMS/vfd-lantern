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
    replace_once(
        "crates/lantern-domain/src/write.rs",
        r#"use crate::{
    DeviceFingerprint, EngineeringValue, MonotonicInstant, OperationId, ParameterId, PlanId,
    RawRegisters, RequestId, SessionId,
};
"#,
        r#"use crate::{
    BackupId, DeviceFingerprint, EngineeringValue, ModbusFunction, MonotonicInstant, OperationId,
    ParameterId, PlanId, RawRegisters, RequestId, SessionId,
};
"#,
    );

    replace_once(
        "crates/lantern-domain/src/write.rs",
        r#"pub struct DeviceWritePreparation {
    pub plan_id: PlanId,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub parameter_id: ParameterId,
    pub context_hash: String,
    pub old_raw: RawRegisters,
    pub target_raw: RawRegisters,
}
"#,
        r#"pub struct DeviceWritePreparation {
    pub plan_id: PlanId,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub parameter_id: ParameterId,
    pub context_hash: String,
    pub old_raw: RawRegisters,
    pub old_engineering: EngineeringValue,
    pub target_raw: RawRegisters,
    pub target_engineering: EngineeringValue,
    pub write_function: ModbusFunction,
}
"#,
    );

    let domain_write = fs::read_to_string("crates/lantern-domain/src/write.rs").expect("domain write");
    assert!(!domain_write.contains("pub struct OperationAuditStart"));
    fs::write(
        "crates/lantern-domain/src/write.rs",
        format!(
            "{domain_write}\n{}",
            r#"
/// Durable start record for a guarded multi-step operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationAuditStart {
    pub operation_id: OperationId,
    pub backup_id: BackupId,
    pub plan_hash: String,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub at: MonotonicInstant,
}

/// Final state recorded for a guarded multi-step operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationAuditOutcome {
    Completed,
    Aborted,
}

/// Durable finish/abort record for a guarded multi-step operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationAuditFinish {
    pub outcome: OperationAuditOutcome,
    pub final_step_index: Option<usize>,
    pub summary: String,
    pub at: MonotonicInstant,
}

/// Single-use proof that an operation start is already durable.
#[derive(Debug, Eq, PartialEq)]
pub struct OperationToken {
    token_id: u128,
    operation_id: OperationId,
    backup_id: BackupId,
    plan_hash: String,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    profile_hash: String,
}

impl OperationToken {
    #[must_use]
    pub fn for_start(token_id: u128, start: &OperationAuditStart) -> Self {
        Self {
            token_id,
            operation_id: start.operation_id,
            backup_id: start.backup_id,
            plan_hash: start.plan_hash.clone(),
            session_id: start.session_id,
            fingerprint: start.fingerprint.clone(),
            profile_hash: start.profile_hash.clone(),
        }
    }

    #[must_use]
    pub const fn token_id(&self) -> u128 {
        self.token_id
    }

    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn backup_id(&self) -> BackupId {
        self.backup_id
    }

    #[must_use]
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &DeviceFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn profile_hash(&self) -> &str {
        &self.profile_hash
    }

    #[must_use]
    pub fn matches_start(&self, start: &OperationAuditStart) -> bool {
        self.operation_id == start.operation_id
            && self.backup_id == start.backup_id
            && self.plan_hash == start.plan_hash
            && self.session_id == start.session_id
            && self.fingerprint == start.fingerprint
            && self.profile_hash == start.profile_hash
    }
}
"#
        ),
    )
    .expect("append operation audit types");

    replace_once(
        "crates/lantern-domain/src/lib.rs",
        r#"pub use write::{
    DecisionAuditRecord, DecisionOutcome, DeviceWriteOutcome, DeviceWritePreparation, PreparedToken,
    ReadBackEvidence, ReadBackOutcome, WriteIntent, WriteOutcome,
};
"#,
        r#"pub use write::{
    DecisionAuditRecord, DecisionOutcome, DeviceWriteOutcome, DeviceWritePreparation,
    OperationAuditFinish, OperationAuditOutcome, OperationAuditStart, OperationToken, PreparedToken,
    ReadBackEvidence, ReadBackOutcome, WriteIntent, WriteOutcome,
};
"#,
    );

    replace_once(
        "crates/lantern-app/src/ports.rs",
        r#"use lantern_domain::{
    DecisionAuditRecord, DeviceFingerprint, DeviceWriteOutcome, DeviceWritePreparation, DriveState,
    PreparedToken, ProfileId, ReadBackEvidence, SessionId, SlaveId, WriteOutcome,
};
"#,
        r#"use lantern_domain::{
    DecisionAuditRecord, DeviceFingerprint, DeviceWriteOutcome, DeviceWritePreparation, DriveState,
    OperationAuditFinish, OperationAuditStart, OperationToken, PreparedToken, ProfileId,
    ReadBackEvidence, SessionId, SlaveId, WriteOutcome,
};
"#,
    );

    replace_once(
        "crates/lantern-app/src/ports.rs",
        r#"    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }
}
"#,
        r#"    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn begin_operation(
        &self,
        _start: OperationAuditStart,
    ) -> PortFuture<'_, Result<OperationToken, AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn finish_operation(
        &self,
        _token: OperationToken,
        _finish: OperationAuditFinish,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }
}
"#,
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        r#"            context_hash: plan.context_hash.clone(),
            old_raw: final_old,
            target_raw: plan.target_raw.clone(),
        };
"#,
        r#"            context_hash: plan.context_hash.clone(),
            old_raw: final_old,
            old_engineering: plan.previous_engineering.clone(),
            target_raw: plan.target_raw.clone(),
            target_engineering: plan.requested_engineering.clone(),
            write_function: parameter
                .write_function()
                .expect("manual write parameter was validated with a write function"),
        };
"#,
    );

    replace_once(
        "crates/lantern-storage/src/lib.rs",
        "mod artifacts;\n",
        "mod artifacts;\nmod audit;\n",
    );
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        r#"pub use artifacts::{StorageError, read_bounded, write_new};
"#,
        r#"pub use artifacts::{StorageError, read_bounded, write_new};
pub use audit::{
    AUDIT_SCHEMA_VERSION, AuditStorageError, AuditVerification, FilesystemAuditPort,
    verify_audit_session,
};
"#,
    );

    fs::write(
        "crates/lantern-storage/src/audit.rs",
        r#"use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

use lantern_app::{AuditError, AuditPort, PortFuture};
use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceWriteOutcome, DeviceWritePreparation,
    EngineeringValue, ModbusFunction, OperationAuditFinish, OperationAuditOutcome,
    OperationAuditStart, OperationToken, PreparedToken, ReadBackEvidence, SessionId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::atomic::atomic_write;

pub const AUDIT_SCHEMA_VERSION: u32 = 1;
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditVerification {
    ValidFinalized,
    ValidOpen,
    Interrupted,
    RecordChanged,
    RecordMissing,
    TailTruncated,
    HeadMissing,
    HeadMismatch,
    RollbackDetected,
    UnsupportedSchema,
}

#[derive(Debug, Error)]
pub enum AuditStorageError {
    #[error("audit filesystem operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("audit serialization failed: {0}")]
    Serialization(String),
    #[error("audit journal state is inconsistent: {0}")]
    Inconsistent(String),
    #[error("audit token is unknown, consumed, or bound to another operation")]
    InvalidToken,
    #[error("AuditUnavailable must never be recursively persisted")]
    RecursiveAuditUnavailable,
}

impl AuditStorageError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }

    fn audit_error(self) -> AuditError {
        AuditError::Persistence(self.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AuditHead {
    schema_version: u32,
    session_id: String,
    record_count: u64,
    head_hash: String,
    last_time: String,
    open: bool,
    finalized: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AuditEnvelope {
    schema_version: u32,
    sequence: u64,
    session_id: String,
    at: String,
    kind: String,
    previous_hash: Option<String>,
    body: Value,
    record_hash: String,
}

#[derive(Serialize)]
struct HashMaterial<'a> {
    schema_version: u32,
    sequence: u64,
    session_id: &'a str,
    at: &'a str,
    kind: &'a str,
    previous_hash: &'a Option<String>,
    body: &'a Value,
}

#[derive(Clone)]
struct PreparedBinding {
    plan_id: u128,
    request_id: u64,
    context_hash: String,
}

#[derive(Clone)]
struct OperationBinding {
    operation_id: u128,
    backup_id: u128,
    plan_hash: String,
    session_id: u128,
    fingerprint: String,
    profile_hash: String,
}

struct AuditState {
    next_token_id: u128,
    prepared: BTreeMap<u128, PreparedBinding>,
    operations: BTreeMap<u128, OperationBinding>,
}

/// Production filesystem implementation of the durable audit boundary.
pub struct FilesystemAuditPort {
    root: PathBuf,
    state: Mutex<AuditState>,
}

impl FilesystemAuditPort {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, AuditStorageError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| AuditStorageError::io(&root, error))?;
        fs::set_permissions(&root, Permissions::from_mode(PRIVATE_DIR_MODE))
            .map_err(|error| AuditStorageError::io(&root, error))?;
        Ok(Self {
            root,
            state: Mutex::new(AuditState {
                next_token_id: 1,
                prepared: BTreeMap::new(),
                operations: BTreeMap::new(),
            }),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn finalize_session(&self, session_id: SessionId) -> Result<(), AuditStorageError> {
        let head_path = head_path(&self.root, session_id);
        let mut head = read_head(&head_path)?
            .ok_or_else(|| AuditStorageError::Inconsistent("cannot finalize missing head".into()))?;
        if head.schema_version != AUDIT_SCHEMA_VERSION {
            return Err(AuditStorageError::Inconsistent(
                "cannot finalize unsupported audit schema".into(),
            ));
        }
        head.open = false;
        head.finalized = true;
        write_head(&head_path, &head)
    }

    fn allocate_token_id(&self) -> u128 {
        let mut state = self.state.lock().expect("audit token state poisoned");
        let id = state.next_token_id;
        state.next_token_id = state.next_token_id.saturating_add(1);
        id
    }

    fn append(
        &self,
        session_id: SessionId,
        at: u128,
        kind: &str,
        body: Value,
    ) -> Result<(), AuditStorageError> {
        append_record(&self.root, session_id, at, kind, body)
    }
}

impl AuditPort for FilesystemAuditPort {
    fn is_available(&self) -> bool {
        fs::metadata(&self.root).is_ok_and(|metadata| metadata.is_dir())
    }

    fn record_decision(
        &self,
        record: DecisionAuditRecord,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        let result = if record.decision == DecisionOutcome::AuditUnavailable {
            Err(AuditStorageError::RecursiveAuditUnavailable)
        } else {
            self.append(
                record.session_id,
                record.at.as_nanos(),
                "decision",
                decision_body(&record),
            )
        }
        .map_err(AuditStorageError::audit_error);
        Box::pin(async move { result })
    }

    fn prepare_device_write(
        &self,
        preparation: DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        let token_id = self.allocate_token_id();
        let result = self
            .append(
                preparation.session_id,
                0,
                "device_write_prepared",
                preparation_body(token_id, &preparation),
            )
            .map(|()| {
                let binding = PreparedBinding {
                    plan_id: preparation.plan_id.get(),
                    request_id: preparation.request_id.get(),
                    context_hash: preparation.context_hash.clone(),
                };
                self.state
                    .lock()
                    .expect("audit token state poisoned")
                    .prepared
                    .insert(token_id, binding);
                PreparedToken::for_preparation(token_id, &preparation)
            })
            .map_err(AuditStorageError::audit_error);
        Box::pin(async move { result })
    }

    fn finalize_device_write(
        &self,
        token: PreparedToken,
        outcome: DeviceWriteOutcome,
        read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        let binding = self
            .state
            .lock()
            .expect("audit token state poisoned")
            .prepared
            .remove(&token.token_id());
        let result = match binding {
            Some(binding)
                if binding.plan_id == token.plan_id().get()
                    && binding.request_id == token.request_id().get()
                    && binding.context_hash == token.context_hash() =>
            {
                let session_id = session_for_prepared_record(&self.root, token.token_id())
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
            }
            _ => Err(AuditStorageError::InvalidToken),
        }
        .map_err(AuditStorageError::audit_error);
        Box::pin(async move { result })
    }

    fn begin_operation(
        &self,
        start: OperationAuditStart,
    ) -> PortFuture<'_, Result<OperationToken, AuditError>> {
        let token_id = self.allocate_token_id();
        let result = self
            .append(
                start.session_id,
                start.at.as_nanos(),
                "operation_started",
                operation_start_body(token_id, &start),
            )
            .map(|()| {
                self.state
                    .lock()
                    .expect("audit token state poisoned")
                    .operations
                    .insert(
                        token_id,
                        OperationBinding {
                            operation_id: start.operation_id.get(),
                            backup_id: start.backup_id.get(),
                            plan_hash: start.plan_hash.clone(),
                            session_id: start.session_id.get(),
                            fingerprint: start.fingerprint.as_str().to_owned(),
                            profile_hash: start.profile_hash.clone(),
                        },
                    );
                OperationToken::for_start(token_id, &start)
            })
            .map_err(AuditStorageError::audit_error);
        Box::pin(async move { result })
    }

    fn finish_operation(
        &self,
        token: OperationToken,
        finish: OperationAuditFinish,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        let binding = self
            .state
            .lock()
            .expect("audit token state poisoned")
            .operations
            .remove(&token.token_id());
        let result = match binding {
            Some(binding)
                if binding.operation_id == token.operation_id().get()
                    && binding.backup_id == token.backup_id().get()
                    && binding.plan_hash == token.plan_hash()
                    && binding.session_id == token.session_id().get()
                    && binding.fingerprint == token.fingerprint().as_str()
                    && binding.profile_hash == token.profile_hash() =>
            {
                self.append(
                    token.session_id(),
                    finish.at.as_nanos(),
                    "operation_finished",
                    operation_finish_body(token.token_id(), &finish),
                )
            }
            _ => Err(AuditStorageError::InvalidToken),
        }
        .map_err(AuditStorageError::audit_error);
        Box::pin(async move { result })
    }
}

fn journal_path(root: &Path, session_id: SessionId) -> PathBuf {
    root.join(format!("audit_{}.jsonl", session_id.get()))
}

fn head_path(root: &Path, session_id: SessionId) -> PathBuf {
    root.join(format!("audit_{}.head.json", session_id.get()))
}

fn read_head(path: &Path) -> Result<Option<AuditHead>, AuditStorageError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AuditStorageError::Serialization(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AuditStorageError::io(path, error)),
    }
}

fn write_head(path: &Path, head: &AuditHead) -> Result<(), AuditStorageError> {
    let bytes = serde_jcs::to_vec(head)
        .map_err(|error| AuditStorageError::Serialization(error.to_string()))?;
    atomic_write(path, &bytes).map_err(|error| AuditStorageError::Inconsistent(error.to_string()))
}

fn append_record(
    root: &Path,
    session_id: SessionId,
    at: u128,
    kind: &str,
    body: Value,
) -> Result<(), AuditStorageError> {
    let journal = journal_path(root, session_id);
    let head_file = head_path(root, session_id);
    let existing_head = read_head(&head_file)?;
    if existing_head.is_none()
        && fs::metadata(&journal).is_ok_and(|metadata| metadata.len() != 0)
    {
        return Err(AuditStorageError::Inconsistent(
            "journal exists without its durable head".into(),
        ));
    }
    if existing_head
        .as_ref()
        .is_some_and(|head| !head.open || head.finalized)
    {
        return Err(AuditStorageError::Inconsistent(
            "cannot append to finalized audit session".into(),
        ));
    }
    if existing_head
        .as_ref()
        .is_some_and(|head| head.schema_version != AUDIT_SCHEMA_VERSION)
    {
        return Err(AuditStorageError::Inconsistent(
            "unsupported existing audit schema".into(),
        ));
    }

    let sequence = existing_head
        .as_ref()
        .map_or(1, |head| head.record_count.saturating_add(1));
    let previous_hash = existing_head.as_ref().map(|head| head.head_hash.clone());
    let session_text = session_id.get().to_string();
    let at_text = at.to_string();
    let material = HashMaterial {
        schema_version: AUDIT_SCHEMA_VERSION,
        sequence,
        session_id: &session_text,
        at: &at_text,
        kind,
        previous_hash: &previous_hash,
        body: &body,
    };
    let record_hash = hash_jcs(&material)?;
    let envelope = AuditEnvelope {
        schema_version: AUDIT_SCHEMA_VERSION,
        sequence,
        session_id: session_text.clone(),
        at: at_text.clone(),
        kind: kind.to_owned(),
        previous_hash,
        body,
        record_hash: record_hash.clone(),
    };
    let mut bytes = serde_jcs::to_vec(&envelope)
        .map_err(|error| AuditStorageError::Serialization(error.to_string()))?;
    bytes.push(b'\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&journal)
        .map_err(|error| AuditStorageError::io(&journal, error))?;
    file.set_permissions(Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|error| AuditStorageError::io(&journal, error))?;
    file.write_all(&bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| AuditStorageError::io(&journal, error))?;

    let head = AuditHead {
        schema_version: AUDIT_SCHEMA_VERSION,
        session_id: session_text,
        record_count: sequence,
        head_hash: record_hash,
        last_time: at_text,
        open: true,
        finalized: false,
    };
    write_head(&head_file, &head)
}

fn hash_jcs(value: &impl Serialize) -> Result<String, AuditStorageError> {
    let canonical = serde_jcs::to_vec(value)
        .map_err(|error| AuditStorageError::Serialization(error.to_string()))?;
    let digest = Sha256::digest(canonical);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn decision_body(record: &DecisionAuditRecord) -> Value {
    json!({
        "plan_id": record.plan_id.get().to_string(),
        "fingerprint": record.fingerprint.as_str(),
        "profile_hash": record.profile_hash,
        "parameter_id": record.parameter_id.as_str(),
        "context_hash": record.context_hash,
        "decision": decision_name(record.decision),
    })
}

fn preparation_body(token_id: u128, preparation: &DeviceWritePreparation) -> Value {
    json!({
        "token_id": token_id.to_string(),
        "plan_id": preparation.plan_id.get().to_string(),
        "operation_id": preparation.operation_id.get().to_string(),
        "request_id": preparation.request_id.get(),
        "fingerprint": preparation.fingerprint.as_str(),
        "profile_hash": preparation.profile_hash,
        "parameter_id": preparation.parameter_id.as_str(),
        "context_hash": preparation.context_hash,
        "old_raw": preparation.old_raw.as_slice(),
        "old_engineering": engineering_json(&preparation.old_engineering),
        "target_raw": preparation.target_raw.as_slice(),
        "target_engineering": engineering_json(&preparation.target_engineering),
        "write_function": function_name(preparation.write_function),
    })
}

fn finalize_body(
    token_id: u128,
    outcome: DeviceWriteOutcome,
    read_back: &ReadBackEvidence,
) -> Value {
    json!({
        "token_id": token_id.to_string(),
        "outcome": device_outcome_name(outcome),
        "read_back": read_back_json(read_back),
    })
}

fn operation_start_body(token_id: u128, start: &OperationAuditStart) -> Value {
    json!({
        "token_id": token_id.to_string(),
        "operation_id": start.operation_id.get().to_string(),
        "backup_id": start.backup_id.get().to_string(),
        "plan_hash": start.plan_hash,
        "fingerprint": start.fingerprint.as_str(),
        "profile_hash": start.profile_hash,
    })
}

fn operation_finish_body(token_id: u128, finish: &OperationAuditFinish) -> Value {
    json!({
        "token_id": token_id.to_string(),
        "outcome": match finish.outcome {
            OperationAuditOutcome::Completed => "completed",
            OperationAuditOutcome::Aborted => "aborted",
        },
        "final_step_index": finish.final_step_index,
        "summary": finish.summary,
    })
}

fn engineering_json(value: &EngineeringValue) -> Value {
    match value {
        EngineeringValue::Fixed(value) => json!({
            "kind": "fixed",
            "text": value.normalize().to_string(),
        }),
        EngineeringValue::Float32Bits(bits) => json!({
            "kind": "float32",
            "bits": format!("{bits:08x}"),
            "text": f32::from_bits(*bits).to_string(),
        }),
        EngineeringValue::Float64Bits(bits) => json!({
            "kind": "float64",
            "bits": format!("{bits:016x}"),
            "text": f64::from_bits(*bits).to_string(),
        }),
        EngineeringValue::EnumRaw(raw) => json!({"kind": "enum", "raw": raw}),
        EngineeringValue::BitfieldRaw(raw) => json!({"kind": "bitfield", "raw": raw}),
    }
}

fn read_back_json(value: &ReadBackEvidence) -> Value {
    match value {
        ReadBackEvidence::NotAttempted => json!({"kind": "not_attempted"}),
        ReadBackEvidence::Verified { attempts, raw } => json!({
            "kind": "verified",
            "attempts": attempts,
            "raw": raw.as_slice(),
        }),
        ReadBackEvidence::Mismatch { attempts, last_raw } => json!({
            "kind": "mismatch",
            "attempts": attempts,
            "last_raw": last_raw.as_slice(),
        }),
        ReadBackEvidence::Unavailable { attempts, reason } => json!({
            "kind": "unavailable",
            "attempts": attempts,
            "reason": reason,
        }),
    }
}

const fn decision_name(value: DecisionOutcome) -> &'static str {
    match value {
        DecisionOutcome::Expired => "expired",
        DecisionOutcome::Cancelled => "cancelled",
        DecisionOutcome::RejectedByPolicy => "rejected_by_policy",
        DecisionOutcome::ProfileNotTrusted => "profile_not_trusted",
        DecisionOutcome::PreconditionChanged => "precondition_changed",
        DecisionOutcome::AuditUnavailable => "audit_unavailable",
    }
}

const fn device_outcome_name(value: DeviceWriteOutcome) -> &'static str {
    match value {
        DeviceWriteOutcome::Verified => "verified",
        DeviceWriteOutcome::DeviceRejected => "device_rejected",
        DeviceWriteOutcome::ReadBackMismatch => "read_back_mismatch",
        DeviceWriteOutcome::OutcomeUnknown => "outcome_unknown",
        DeviceWriteOutcome::TransportLost => "transport_lost",
        DeviceWriteOutcome::AuditDegraded => "audit_degraded",
    }
}

const fn function_name(value: ModbusFunction) -> &'static str {
    match value {
        ModbusFunction::ReadHoldingRegisters => "fc03",
        ModbusFunction::ReadInputRegisters => "fc04",
        ModbusFunction::WriteSingleRegister => "fc06",
        ModbusFunction::WriteMultipleRegisters => "fc16",
    }
}

fn session_for_prepared_record(root: &Path, token_id: u128) -> Option<SessionId> {
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

pub fn verify_audit_session(root: &Path, session_id: SessionId) -> AuditVerification {
    let head_file = head_path(root, session_id);
    let journal = journal_path(root, session_id);
    let Ok(Some(head)) = read_head(&head_file) else {
        return AuditVerification::HeadMissing;
    };
    if head.schema_version != AUDIT_SCHEMA_VERSION {
        return AuditVerification::UnsupportedSchema;
    }
    if head.session_id != session_id.get().to_string() {
        return AuditVerification::HeadMismatch;
    }
    let Ok(bytes) = fs::read(&journal) else {
        return if head.record_count == 0 {
            AuditVerification::HeadMismatch
        } else {
            AuditVerification::RecordMissing
        };
    };
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return AuditVerification::TailTruncated;
    }

    let mut records = Vec::new();
    let mut expected_previous: Option<String> = None;
    for (index, line) in bytes.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).enumerate() {
        let Ok(record) = serde_json::from_slice::<AuditEnvelope>(line) else {
            return AuditVerification::RecordChanged;
        };
        if record.schema_version != AUDIT_SCHEMA_VERSION {
            return AuditVerification::UnsupportedSchema;
        }
        if record.sequence != (index as u64).saturating_add(1) {
            return AuditVerification::RecordMissing;
        }
        if record.session_id != head.session_id || record.previous_hash != expected_previous {
            return AuditVerification::RecordChanged;
        }
        let material = HashMaterial {
            schema_version: record.schema_version,
            sequence: record.sequence,
            session_id: &record.session_id,
            at: &record.at,
            kind: &record.kind,
            previous_hash: &record.previous_hash,
            body: &record.body,
        };
        let Ok(recomputed) = hash_jcs(&material) else {
            return AuditVerification::RecordChanged;
        };
        if recomputed != record.record_hash {
            return AuditVerification::RecordChanged;
        }
        expected_previous = Some(record.record_hash.clone());
        records.push(record);
    }

    let count = records.len() as u64;
    if count < head.record_count {
        return AuditVerification::RollbackDetected;
    }
    if count > head.record_count {
        if head.record_count == 0
            || records
                .get((head.record_count - 1) as usize)
                .is_some_and(|record| record.record_hash == head.head_hash)
        {
            return AuditVerification::Interrupted;
        }
        return AuditVerification::HeadMismatch;
    }
    if records.last().map(|record| record.record_hash.as_str()) != Some(head.head_hash.as_str()) {
        return AuditVerification::HeadMismatch;
    }
    if head.finalized && !head.open {
        AuditVerification::ValidFinalized
    } else if head.open && !head.finalized {
        AuditVerification::ValidOpen
    } else {
        AuditVerification::HeadMismatch
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use lantern_app::{AuditPort, AuditError};
    use lantern_domain::{
        BackupId, DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
        DeviceWritePreparation, EngineeringValue, ModbusFunction, MonotonicInstant,
        OperationAuditFinish, OperationAuditOutcome, OperationAuditStart, OperationId,
        ParameterId, PlanId, PreparedToken, RawRegisters, ReadBackEvidence, RequestId, SessionId,
    };
    use tempfile::tempdir;

    use super::{
        AuditHead, AuditVerification, FilesystemAuditPort, head_path, journal_path, read_head,
        verify_audit_session, write_head,
    };

    fn fingerprint() -> DeviceFingerprint {
        DeviceFingerprint::parse("audit.test.device").expect("fingerprint")
    }

    fn parameter() -> ParameterId {
        ParameterId::parse("config.acceleration").expect("parameter")
    }

    fn raw(value: u16) -> RawRegisters {
        RawRegisters::new(vec![value]).expect("raw")
    }

    fn preparation(session_id: SessionId) -> DeviceWritePreparation {
        DeviceWritePreparation {
            plan_id: PlanId::new(10),
            operation_id: OperationId::new(11),
            request_id: RequestId::new(12),
            session_id,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            parameter_id: parameter(),
            context_hash: "context-hash".into(),
            old_raw: raw(90),
            old_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(90, 1)),
            target_raw: raw(100),
            target_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(100, 1)),
            write_function: ModbusFunction::WriteSingleRegister,
        }
    }

    #[tokio::test]
    async fn decision_is_durable_without_token_and_recursive_audit_unavailable_is_rejected() {
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new(directory.path()).expect("audit");
        let session = SessionId::new(1);
        audit.record_decision(DecisionAuditRecord {
            plan_id: PlanId::new(1),
            session_id: session,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            parameter_id: parameter(),
            context_hash: None,
            decision: DecisionOutcome::Cancelled,
            at: MonotonicInstant::from_nanos(7),
        }).await.expect("decision");
        assert_eq!(verify_audit_session(directory.path(), session), AuditVerification::ValidOpen);

        let before = fs::read(journal_path(directory.path(), session)).expect("journal");
        let error = audit.record_decision(DecisionAuditRecord {
            plan_id: PlanId::new(2),
            session_id: session,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            parameter_id: parameter(),
            context_hash: None,
            decision: DecisionOutcome::AuditUnavailable,
            at: MonotonicInstant::from_nanos(8),
        }).await.expect_err("must reject recursive audit");
        assert!(matches!(error, AuditError::Persistence(_)));
        assert_eq!(fs::read(journal_path(directory.path(), session)).expect("journal"), before);
    }

    #[tokio::test]
    async fn prepared_and_finalized_device_write_is_bound_single_use_and_private() {
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new(directory.path()).expect("audit");
        let session = SessionId::new(2);
        let preparation = preparation(session);
        let token = audit.prepare_device_write(preparation.clone()).await.expect("prepare");
        let token_id = token.token_id();
        audit.finalize_device_write(
            token,
            DeviceWriteOutcome::Verified,
            ReadBackEvidence::Verified { attempts: 1, raw: raw(100) },
        ).await.expect("finalize");
        assert_eq!(verify_audit_session(directory.path(), session), AuditVerification::ValidOpen);
        let forged = PreparedToken::for_preparation(token_id, &preparation);
        assert!(audit.finalize_device_write(
            forged,
            DeviceWriteOutcome::Verified,
            ReadBackEvidence::NotAttempted,
        ).await.is_err());
        let text = fs::read_to_string(journal_path(directory.path(), session)).expect("journal");
        assert!(text.contains("\"write_function\":\"fc06\""));
        assert!(text.contains("\"old_engineering\""));
        assert_eq!(fs::metadata(journal_path(directory.path(), session)).expect("metadata").permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn operation_token_is_durable_context_bound_and_single_use() {
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new(directory.path()).expect("audit");
        let session = SessionId::new(3);
        let start = OperationAuditStart {
            operation_id: OperationId::new(30),
            backup_id: BackupId::new(31),
            plan_hash: "restore-plan-hash".into(),
            session_id: session,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            at: MonotonicInstant::from_nanos(9),
        };
        let token = audit.begin_operation(start.clone()).await.expect("begin");
        let token_id = token.token_id();
        audit.finish_operation(token, OperationAuditFinish {
            outcome: OperationAuditOutcome::Completed,
            final_step_index: Some(4),
            summary: "all verified".into(),
            at: MonotonicInstant::from_nanos(10),
        }).await.expect("finish");
        let forged = lantern_domain::OperationToken::for_start(token_id, &start);
        assert!(audit.finish_operation(forged, OperationAuditFinish {
            outcome: OperationAuditOutcome::Aborted,
            final_step_index: Some(4),
            summary: "retry forbidden".into(),
            at: MonotonicInstant::from_nanos(11),
        }).await.is_err());
    }

    #[tokio::test]
    async fn verifier_classifies_finalized_interrupted_tampered_truncated_and_rollback() {
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new(directory.path()).expect("audit");
        let session = SessionId::new(4);
        audit.prepare_device_write(preparation(session)).await.expect("prepare");
        let durable_head = fs::read(head_path(directory.path(), session)).expect("head snapshot");
        audit.record_decision(DecisionAuditRecord {
            plan_id: PlanId::new(99),
            session_id: session,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            parameter_id: parameter(),
            context_hash: None,
            decision: DecisionOutcome::Cancelled,
            at: MonotonicInstant::from_nanos(12),
        }).await.expect("second record");
        fs::write(head_path(directory.path(), session), &durable_head).expect("restore stale head");
        assert_eq!(verify_audit_session(directory.path(), session), AuditVerification::Interrupted);

        let current_head: AuditHead = read_head(&head_path(directory.path(), session)).expect("read").expect("head");
        let mut journal = fs::read(journal_path(directory.path(), session)).expect("journal");
        journal.pop();
        fs::write(journal_path(directory.path(), session), &journal).expect("truncate");
        assert_eq!(verify_audit_session(directory.path(), session), AuditVerification::TailTruncated);

        let session2 = SessionId::new(5);
        audit.prepare_device_write(preparation(session2)).await.expect("prepare2");
        let journal2 = journal_path(directory.path(), session2);
        let mut text = fs::read_to_string(&journal2).expect("journal2");
        text = text.replacen("context-hash", "context-Xash", 1);
        fs::write(&journal2, text).expect("tamper");
        assert_eq!(verify_audit_session(directory.path(), session2), AuditVerification::RecordChanged);

        let session3 = SessionId::new(6);
        audit.prepare_device_write(preparation(session3)).await.expect("prepare3");
        audit.record_decision(DecisionAuditRecord {
            plan_id: PlanId::new(100),
            session_id: session3,
            fingerprint: fingerprint(),
            profile_hash: "profile-hash".into(),
            parameter_id: parameter(),
            context_hash: None,
            decision: DecisionOutcome::Cancelled,
            at: MonotonicInstant::from_nanos(13),
        }).await.expect("record3");
        let journal3 = journal_path(directory.path(), session3);
        let text3 = fs::read_to_string(&journal3).expect("journal3");
        let first = text3.lines().next().expect("first");
        fs::write(&journal3, format!("{first}\n")).expect("rollback journal");
        assert_eq!(verify_audit_session(directory.path(), session3), AuditVerification::RollbackDetected);

        let session4 = SessionId::new(7);
        audit.prepare_device_write(preparation(session4)).await.expect("prepare4");
        audit.finalize_session(session4).expect("finalize session");
        assert_eq!(verify_audit_session(directory.path(), session4), AuditVerification::ValidFinalized);
        let _ = current_head;
    }

    #[test]
    fn missing_head_is_never_treated_as_valid() {
        let directory = tempdir().expect("tempdir");
        assert_eq!(
            verify_audit_session(directory.path(), SessionId::new(999)),
            AuditVerification::HeadMissing
        );
    }
}
"#,
    )
    .expect("write audit module");
}
