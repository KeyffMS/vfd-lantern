use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use lantern_domain::Decimal;
use lantern_domain::{
    ByteOrder, DeviceFingerprint, EngineeringValue, ModbusFunction, ModbusTable, ParameterAccess,
    ParameterId, QuantityKind, RawRegisters, RegisterEncoding, RequiredDriveState, RestorePolicy,
    SessionId, TelemetryQuality, WordOrder, WriteIntent,
};
use lantern_profile::{ReadBackPolicy, ValidatedDeviceProfile, ValidatedParameter};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    FrequencyClass, LatestValue, LatestValues, PollPlanError, ProfileOrigin, ReadSubscription,
    SubscriberId, SubscriptionReason,
};

pub const MAX_PARAMETER_BROWSER_VISIBLE: usize = 64;
const PARAMETER_BROWSER_MAXIMUM_AGE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterRiskView {
    ReadOnly,
    Normal,
    Commissioning,
    Dangerous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterEditorKind {
    Fixed,
    Float32,
    Float64,
    Enum,
    Bitfield,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterGroupView {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumOptionView {
    pub raw: i64,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitFlagView {
    pub bit: u8,
    pub label: String,
}

/// Immutable parameter metadata built once for a validated profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterDescriptorView {
    pub parameter_id: ParameterId,
    pub code: String,
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub groups: Vec<ParameterGroupView>,
    pub table: ModbusTable,
    pub pdu_address: u16,
    pub register_count: u16,
    pub source_address_notation: String,
    pub source_address_value: u32,
    pub encoding: RegisterEncoding,
    pub byte_order: ByteOrder,
    pub word_order: WordOrder,
    pub quantity: QuantityKind,
    pub unit: lantern_domain::UnitId,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub step: Option<String>,
    pub access: ParameterAccess,
    pub risk: ParameterRiskView,
    pub restore_policy: RestorePolicy,
    pub required_drive_state: RequiredDriveState,
    pub write_function: Option<ModbusFunction>,
    pub read_back: String,
    pub editor: ParameterEditorKind,
    pub editor_block_reason: Option<String>,
    pub enum_values: Vec<EnumOptionView>,
    pub bit_flags: Vec<BitFlagView>,
    pub search_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParameterProfileView {
    pub profile_id: String,
    pub revision: u32,
    pub vendor: String,
    pub family: String,
    pub model: String,
    pub origin: ProfileOrigin,
    pub profile_hash: String,
    pub source_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedWriteIntent {
    pub intent: WriteIntent,
    pub encoded_engineering: EngineeringValue,
    pub rounded: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParameterBrowserView {
    pub profile: Option<ParameterProfileView>,
    pub catalog: Arc<[ParameterDescriptorView]>,
    pub latest: Option<Arc<LatestValues>>,
    pub staged_intent: Option<StagedWriteIntent>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterEditorInput {
    Fixed(String),
    Float(String),
    Enum(i64),
    Bitfield(u64),
}

#[derive(Clone, Debug)]
pub enum ParameterAction {
    SetVisible(Vec<ParameterId>),
    Refresh(ParameterId),
    PrepareIntent {
        parameter_id: ParameterId,
        input: ParameterEditorInput,
    },
    ClearIntent,
}

#[derive(Clone, Debug)]
pub struct ParameterIntentContext {
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub process_writes_enabled: bool,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ParameterBrowserError {
    #[error("parameter browser requires a Verified connected session")]
    SessionUnavailable,
    #[error("process was started without --enable-writes")]
    ProcessWritesDisabled,
    #[error("parameter {0} is not present in the active validated profile")]
    UnknownParameter(ParameterId),
    #[error("parameter {0} is read-only")]
    ReadOnly(ParameterId),
    #[error("parameter {0} is Dangerous and has no manual editor")]
    Dangerous(ParameterId),
    #[error("parameter {0} has no validated typed editor metadata")]
    EditorUnavailable(ParameterId),
    #[error("parameter {0} does not have a fresh Good last-good value")]
    NotFreshGood(ParameterId),
    #[error("latest telemetry belongs to a different logical session")]
    SessionMismatch,
    #[error("invalid editor input: {0}")]
    InvalidInput(String),
    #[error("requested value is below the profile minimum")]
    BelowMinimum,
    #[error("requested value is above the profile maximum")]
    AboveMaximum,
    #[error("requested value does not satisfy the profile step")]
    StepMismatch,
    #[error("selected enum raw value is not present in the validated profile map")]
    UnknownEnumValue,
    #[error("selected bitfield contains a bit not present in the validated profile map")]
    UnknownBit,
    #[error("requested value cannot be represented by the profile codec: {0}")]
    NotRepresentable(String),
    #[error("encoded raw value is explicitly forbidden by the profile")]
    ForbiddenRaw,
    #[error(transparent)]
    Subscription(#[from] PollPlanError),
}

#[must_use]
pub fn parameter_catalog(profile: &ValidatedDeviceProfile) -> Arc<[ParameterDescriptorView]> {
    let mut groups = BTreeMap::<ParameterId, Vec<ParameterGroupView>>::new();
    for group in profile.groups() {
        for parameter_id in group.parameters.iter() {
            groups
                .entry(parameter_id.clone())
                .or_default()
                .push(ParameterGroupView {
                    id: group.id.clone(),
                    name: group.name.clone(),
                });
        }
    }
    let mut aliases = BTreeMap::<ParameterId, Vec<String>>::new();
    for (alias, parameter_id) in profile.aliases() {
        aliases
            .entry(parameter_id.clone())
            .or_default()
            .push(alias.clone());
    }

    profile
        .parameters()
        .values()
        .map(|parameter| {
            descriptor_from_parameter(
                parameter,
                groups.remove(parameter.id()).unwrap_or_default(),
                aliases.remove(parameter.id()).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .into()
}

fn descriptor_from_parameter(
    parameter: &ValidatedParameter,
    groups: Vec<ParameterGroupView>,
    aliases: Vec<String>,
) -> ParameterDescriptorView {
    let access = parameter.access();
    let risk = match access {
        ParameterAccess::ReadOnly => ParameterRiskView::ReadOnly,
        ParameterAccess::WritableWhenStopped => ParameterRiskView::Normal,
        ParameterAccess::Commissioning => ParameterRiskView::Commissioning,
        ParameterAccess::Dangerous => ParameterRiskView::Dangerous,
    };
    let (editor, editor_block_reason) = editor_kind(parameter);
    let enum_values = parameter
        .enum_values()
        .iter()
        .map(|(raw, label)| EnumOptionView {
            raw: *raw,
            label: label.clone(),
        })
        .collect::<Vec<_>>();
    let bit_flags = parameter
        .bit_flags()
        .iter()
        .map(|(bit, label)| BitFlagView {
            bit: *bit,
            label: label.clone(),
        })
        .collect::<Vec<_>>();
    let block = parameter.block();
    let source_address_notation = parameter.source_address_notation().to_owned();
    let read_back = read_back_label(parameter.read_back());
    let search_text = normalized_search_text(
        parameter,
        &groups,
        &aliases,
        &source_address_notation,
        &read_back,
    );
    ParameterDescriptorView {
        parameter_id: parameter.id().clone(),
        code: parameter.code().to_owned(),
        name: parameter.name().to_owned(),
        description: parameter.description().to_owned(),
        aliases,
        groups,
        table: block.table(),
        pdu_address: block.start().get(),
        register_count: block.count().get(),
        source_address_notation,
        source_address_value: parameter.source_address_value(),
        encoding: parameter.codec().encoding(),
        byte_order: parameter.codec().byte_order(),
        word_order: parameter.codec().word_order(),
        quantity: parameter.quantity().clone(),
        unit: parameter.unit().clone(),
        minimum: parameter
            .minimum()
            .map(|value| value.normalize().to_string()),
        maximum: parameter
            .maximum()
            .map(|value| value.normalize().to_string()),
        step: parameter.step().map(|value| value.normalize().to_string()),
        access,
        risk,
        restore_policy: parameter.restore_policy(),
        required_drive_state: parameter.required_drive_state(),
        write_function: parameter.write_function(),
        read_back,
        editor,
        editor_block_reason,
        enum_values,
        bit_flags,
        search_text,
    }
}

fn editor_kind(parameter: &ValidatedParameter) -> (ParameterEditorKind, Option<String>) {
    match parameter.access() {
        ParameterAccess::ReadOnly => {
            return (
                ParameterEditorKind::Unavailable,
                Some("read-only".to_owned()),
            );
        }
        ParameterAccess::Dangerous => {
            return (
                ParameterEditorKind::Unavailable,
                Some("Dangerous has no manual editor".to_owned()),
            );
        }
        ParameterAccess::WritableWhenStopped | ParameterAccess::Commissioning => {}
    }
    if parameter.write_function().is_none() {
        return (
            ParameterEditorKind::Unavailable,
            Some("profile defines no write function".to_owned()),
        );
    }
    match parameter.codec().encoding() {
        RegisterEncoding::Unsigned16
        | RegisterEncoding::Signed16
        | RegisterEncoding::Unsigned32
        | RegisterEncoding::Signed32
        | RegisterEncoding::Unsigned64
        | RegisterEncoding::Signed64
        | RegisterEncoding::Bcd16
        | RegisterEncoding::Bcd32 => (ParameterEditorKind::Fixed, None),
        RegisterEncoding::Float32 => (ParameterEditorKind::Float32, None),
        RegisterEncoding::Float64 => (ParameterEditorKind::Float64, None),
        RegisterEncoding::Enum16 | RegisterEncoding::Enum32
            if !parameter.enum_values().is_empty() =>
        {
            (ParameterEditorKind::Enum, None)
        }
        RegisterEncoding::Enum16 | RegisterEncoding::Enum32 => (
            ParameterEditorKind::Unavailable,
            Some("enum profile map is empty; free-text raw is forbidden".to_owned()),
        ),
        RegisterEncoding::Bitfield16
        | RegisterEncoding::Bitfield32
        | RegisterEncoding::Bitfield64
            if !parameter.bit_flags().is_empty() =>
        {
            (ParameterEditorKind::Bitfield, None)
        }
        RegisterEncoding::Bitfield16
        | RegisterEncoding::Bitfield32
        | RegisterEncoding::Bitfield64 => (
            ParameterEditorKind::Unavailable,
            Some("bitfield profile map is empty; free-text raw is forbidden".to_owned()),
        ),
    }
}

fn normalized_search_text(
    parameter: &ValidatedParameter,
    groups: &[ParameterGroupView],
    aliases: &[String],
    source_address_notation: &str,
    read_back: &str,
) -> String {
    let mut fields = vec![
        parameter.id().as_str().to_owned(),
        parameter.code().to_owned(),
        parameter.name().to_owned(),
        parameter.description().to_owned(),
        format!("{:?}", parameter.quantity()),
        parameter.unit().as_str().to_owned(),
        format!("{:?}", parameter.access()),
        format!("{:?}", parameter.restore_policy()),
        format!("{:?}", parameter.codec().encoding()),
        source_address_notation.to_owned(),
        read_back.to_owned(),
    ];
    fields.extend(aliases.iter().cloned());
    for group in groups {
        fields.push(group.id.clone());
        fields.push(group.name.clone());
    }
    fields.join(" ").to_ascii_lowercase()
}

fn read_back_label(policy: &ReadBackPolicy) -> String {
    match policy {
        ReadBackPolicy::ExactRaw => "exact_raw".to_owned(),
        ReadBackPolicy::AcceptedRawSet(values) => format!("accepted_raw_set({})", values.len()),
        ReadBackPolicy::FloatExactBits => "float_exact_bits".to_owned(),
        ReadBackPolicy::FloatAbsRelTolerance { absolute, relative } => {
            format!("float_abs_rel_tolerance(abs={absolute},rel={relative})")
        }
    }
}

#[must_use]
pub fn project_parameter_browser_view(
    profile: &ValidatedDeviceProfile,
    origin: ProfileOrigin,
    catalog: Arc<[ParameterDescriptorView]>,
    latest: Option<Arc<LatestValues>>,
    staged_intent: Option<StagedWriteIntent>,
    error: Option<&str>,
) -> ParameterBrowserView {
    ParameterBrowserView {
        profile: Some(ParameterProfileView {
            profile_id: profile.profile_id().as_str().to_owned(),
            revision: profile.revision(),
            vendor: profile.vendor().to_owned(),
            family: profile.family().to_owned(),
            model: profile.model().to_owned(),
            origin,
            profile_hash: profile.profile_hash().to_hex(),
            source_hash: profile.source_hash().to_hex(),
        }),
        catalog,
        latest,
        staged_intent,
        error: error.map(str::to_owned),
    }
}

pub fn parameter_browser_subscriptions(
    profile: &ValidatedDeviceProfile,
    visible: &[ParameterId],
) -> Result<Vec<ReadSubscription>, ParameterBrowserError> {
    let mut unique = BTreeSet::new();
    let mut result = Vec::new();
    for parameter_id in visible.iter().take(MAX_PARAMETER_BROWSER_VISIBLE) {
        if profile.parameter(parameter_id).is_none() {
            return Err(ParameterBrowserError::UnknownParameter(
                parameter_id.clone(),
            ));
        }
        if !unique.insert(parameter_id.clone()) {
            continue;
        }
        result.push(ReadSubscription::new(
            parameter_id.clone(),
            FrequencyClass::Slow,
            parameter_subscriber_id("parameters", parameter_id)?,
            SubscriptionReason::ParameterBrowser,
            false,
            PARAMETER_BROWSER_MAXIMUM_AGE,
        )?);
    }
    Ok(result)
}

pub fn parameter_refresh_subscription(
    profile: &ValidatedDeviceProfile,
    parameter_id: &ParameterId,
) -> Result<ReadSubscription, ParameterBrowserError> {
    if profile.parameter(parameter_id).is_none() {
        return Err(ParameterBrowserError::UnknownParameter(
            parameter_id.clone(),
        ));
    }
    Ok(ReadSubscription::new(
        parameter_id.clone(),
        FrequencyClass::Fast,
        parameter_subscriber_id("parameters-refresh", parameter_id)?,
        SubscriptionReason::ParameterBrowser,
        false,
        Duration::from_millis(500),
    )?)
}

fn parameter_subscriber_id(
    prefix: &str,
    parameter_id: &ParameterId,
) -> Result<SubscriberId, ParameterBrowserError> {
    let digest = Sha256::digest(parameter_id.as_str().as_bytes());
    let token = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(SubscriberId::parse(format!("{prefix}:{token}"))?)
}

pub fn prepare_parameter_intent(
    profile: &ValidatedDeviceProfile,
    latest: &LatestValues,
    context: ParameterIntentContext,
    parameter_id: &ParameterId,
    input: &ParameterEditorInput,
) -> Result<StagedWriteIntent, ParameterBrowserError> {
    if !context.process_writes_enabled {
        return Err(ParameterBrowserError::ProcessWritesDisabled);
    }
    if latest.session_id() != context.session_id {
        return Err(ParameterBrowserError::SessionMismatch);
    }
    let parameter = profile
        .parameter(parameter_id)
        .ok_or_else(|| ParameterBrowserError::UnknownParameter(parameter_id.clone()))?;
    match parameter.access() {
        ParameterAccess::ReadOnly => {
            return Err(ParameterBrowserError::ReadOnly(parameter_id.clone()));
        }
        ParameterAccess::Dangerous => {
            return Err(ParameterBrowserError::Dangerous(parameter_id.clone()));
        }
        ParameterAccess::WritableWhenStopped | ParameterAccess::Commissioning => {}
    }
    let (editor, _) = editor_kind(parameter);
    if editor == ParameterEditorKind::Unavailable {
        return Err(ParameterBrowserError::EditorUnavailable(
            parameter_id.clone(),
        ));
    }
    let current = latest
        .value(parameter_id)
        .filter(|value| value.can_satisfy_write_guard())
        .ok_or_else(|| ParameterBrowserError::NotFreshGood(parameter_id.clone()))?;
    let previous = current
        .last_good
        .as_ref()
        .ok_or_else(|| ParameterBrowserError::NotFreshGood(parameter_id.clone()))?;

    let requested = parse_editor_value(parameter, input)?;
    validate_constraints(parameter, &requested)?;
    let encoded = parameter
        .codec()
        .encode(&requested)
        .map_err(|error| ParameterBrowserError::NotRepresentable(error.to_string()))?;
    let preview_raw = RawRegisters::new(encoded)
        .map_err(|error| ParameterBrowserError::NotRepresentable(error.to_string()))?;
    if parameter
        .forbidden_raw()
        .iter()
        .any(|forbidden| forbidden == &preview_raw)
    {
        return Err(ParameterBrowserError::ForbiddenRaw);
    }
    let encoded_engineering = parameter
        .codec()
        .decode(preview_raw.as_slice())
        .map_err(|error| ParameterBrowserError::NotRepresentable(error.to_string()))?;
    let rounded = encoded_engineering != requested;
    let intent = WriteIntent {
        session_id: context.session_id,
        fingerprint: context.fingerprint,
        profile_hash: context.profile_hash,
        parameter_id: parameter_id.clone(),
        previous_raw: previous.raw.clone(),
        previous_engineering: previous.engineering.clone(),
        previous_observed_at: previous.monotonic_time,
        requested_engineering: requested,
        preview_raw: Some(preview_raw),
        created_at: latest.captured_at(),
    };
    Ok(StagedWriteIntent {
        intent,
        encoded_engineering,
        rounded,
    })
}

fn parse_editor_value(
    parameter: &ValidatedParameter,
    input: &ParameterEditorInput,
) -> Result<EngineeringValue, ParameterBrowserError> {
    match (parameter.codec().encoding(), input) {
        (
            RegisterEncoding::Unsigned16
            | RegisterEncoding::Signed16
            | RegisterEncoding::Unsigned32
            | RegisterEncoding::Signed32
            | RegisterEncoding::Unsigned64
            | RegisterEncoding::Signed64
            | RegisterEncoding::Bcd16
            | RegisterEncoding::Bcd32,
            ParameterEditorInput::Fixed(text),
        ) => text
            .trim()
            .parse::<Decimal>()
            .map(EngineeringValue::Fixed)
            .map_err(|error| ParameterBrowserError::InvalidInput(error.to_string())),
        (RegisterEncoding::Float32, ParameterEditorInput::Float(text)) => {
            let value = text
                .trim()
                .parse::<f32>()
                .map_err(|error| ParameterBrowserError::InvalidInput(error.to_string()))?;
            if !value.is_finite() {
                return Err(ParameterBrowserError::InvalidInput(
                    "float input must be finite".to_owned(),
                ));
            }
            Ok(EngineeringValue::Float32Bits(value.to_bits()))
        }
        (RegisterEncoding::Float64, ParameterEditorInput::Float(text)) => {
            let value = text
                .trim()
                .parse::<f64>()
                .map_err(|error| ParameterBrowserError::InvalidInput(error.to_string()))?;
            if !value.is_finite() {
                return Err(ParameterBrowserError::InvalidInput(
                    "float input must be finite".to_owned(),
                ));
            }
            Ok(EngineeringValue::Float64Bits(value.to_bits()))
        }
        (RegisterEncoding::Enum16 | RegisterEncoding::Enum32, ParameterEditorInput::Enum(raw)) => {
            if !parameter.enum_values().contains_key(raw) {
                return Err(ParameterBrowserError::UnknownEnumValue);
            }
            Ok(EngineeringValue::EnumRaw(*raw))
        }
        (
            RegisterEncoding::Bitfield16
            | RegisterEncoding::Bitfield32
            | RegisterEncoding::Bitfield64,
            ParameterEditorInput::Bitfield(raw),
        ) => {
            let allowed = parameter
                .bit_flags()
                .keys()
                .fold(0_u64, |mask, bit| mask | (1_u64 << u32::from(*bit)));
            if raw & !allowed != 0 {
                return Err(ParameterBrowserError::UnknownBit);
            }
            Ok(EngineeringValue::BitfieldRaw(*raw))
        }
        _ => Err(ParameterBrowserError::InvalidInput(
            "editor input kind does not match the active profile encoding".to_owned(),
        )),
    }
}

fn validate_constraints(
    parameter: &ValidatedParameter,
    value: &EngineeringValue,
) -> Result<(), ParameterBrowserError> {
    let EngineeringValue::Fixed(value) = value else {
        return Ok(());
    };
    if parameter.minimum().is_some_and(|minimum| *value < minimum) {
        return Err(ParameterBrowserError::BelowMinimum);
    }
    if parameter.maximum().is_some_and(|maximum| *value > maximum) {
        return Err(ParameterBrowserError::AboveMaximum);
    }
    if let Some(step) = parameter.step() {
        let origin = parameter.minimum().unwrap_or(Decimal::ZERO);
        let delta = value.checked_sub(origin).ok_or_else(|| {
            ParameterBrowserError::InvalidInput("range arithmetic overflow".to_owned())
        })?;
        if delta % step != Decimal::ZERO {
            return Err(ParameterBrowserError::StepMismatch);
        }
    }
    Ok(())
}

#[must_use]
pub fn parameter_quality(latest: Option<&LatestValue>) -> TelemetryQuality {
    latest.map_or(TelemetryQuality::Unavailable, |value| value.current_quality)
}

#[cfg(test)]
mod tests {
    use lantern_domain::{ParameterAccess, RegisterEncoding};
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use super::{ParameterEditorKind, parameter_catalog};

    #[test]
    fn read_only_catalog_is_metadata_only_and_has_no_editor() {
        let profile = parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile");
        let catalog = parameter_catalog(&profile);
        assert!(!catalog.is_empty());
        assert!(
            catalog
                .iter()
                .any(|entry| entry.access == ParameterAccess::ReadOnly)
        );
        assert!(
            catalog
                .iter()
                .filter(|entry| entry.access == ParameterAccess::ReadOnly)
                .all(|entry| entry.editor == ParameterEditorKind::Unavailable)
        );
        assert!(
            catalog
                .iter()
                .filter(|entry| entry.access == ParameterAccess::Dangerous)
                .all(|entry| entry.editor == ParameterEditorKind::Unavailable)
        );
        assert!(
            catalog
                .iter()
                .any(|entry| entry.encoding == RegisterEncoding::Unsigned16)
        );
    }
}
