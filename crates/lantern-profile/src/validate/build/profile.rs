use super::super::{helpers::*, references::*, *};
use super::{parameter::validate_parameter, protocol::validate_protocol};

pub(super) fn validate_profile(
    mut document: ProfileDocumentV1,
    source_hash: SourceHash,
) -> Result<ValidatedDeviceProfile, ProfileError> {
    if document.schema_version != 1 {
        return Err(ProfileError::UnsupportedSchema(document.schema_version));
    }
    if document.revision == 0 {
        return Err(ProfileError::validation(
            "revision",
            "revision must be non-zero",
        ));
    }
    validate_text("vendor", &document.vendor, false)?;
    validate_text("family", &document.family, false)?;
    validate_text("model", &document.model, false)?;
    for (index, source) in document.sources.iter().enumerate() {
        validate_text(format!("sources[{index}]"), source, false)?;
    }
    for (index, note) in document.safety_notes.iter().enumerate() {
        validate_text(format!("safety_notes[{index}]"), note, false)?;
    }

    let profile_id = ProfileId::parse(document.profile_id.clone())
        .map_err(|error| ProfileError::validation("profile_id", error))?;
    let protocol = validate_protocol(&mut document)?;

    if document.parameters.len() > MAX_PARAMETERS {
        return Err(ProfileError::validation(
            "parameters",
            format!(
                "contains {} entries; maximum is {MAX_PARAMETERS}",
                document.parameters.len()
            ),
        ));
    }

    let mut parameter_ids = BTreeSet::new();
    let mut parameter_codes = BTreeSet::new();
    let mut parameters = BTreeMap::new();
    for (index, parameter) in document.parameters.iter_mut().enumerate() {
        let validated = validate_parameter(parameter, index)?;
        if !parameter_ids.insert(validated.id.clone()) {
            return Err(ProfileError::validation(
                format!("parameters[{index}].id"),
                "duplicate parameter ID",
            ));
        }
        if !parameter_codes.insert(validated.code.clone()) {
            return Err(ProfileError::validation(
                format!("parameters[{index}].code"),
                "duplicate parameter code",
            ));
        }
        parameters.insert(validated.id.clone(), validated);
    }
    document
        .parameters
        .sort_by(|left, right| left.id.cmp(&right.id));

    let probes = validate_probes(&mut document, &parameters)?;
    let aliases = validate_aliases(&document, &parameters)?;
    let groups = validate_groups(&document, &parameters)?;
    let (fault_source, faults) = validate_faults(&document, &parameters)?;
    let presets = validate_presets(&document, &parameters)?;
    let restore_order = validate_restore_order(&document, &parameters)?;

    document.sources.sort();
    document.sources.dedup();

    let canonical = CanonicalProfileV1 {
        canonical_schema_version: 1,
        profile: &document,
    };
    let canonical_bytes = serde_jcs::to_vec(&canonical)
        .map_err(|error| ProfileError::Canonical(error.to_string()))?;
    let profile_hash = ProfileHash::digest(&canonical_bytes);

    Ok(ValidatedDeviceProfile {
        profile_id,
        revision: document.revision,
        vendor: document.vendor.clone(),
        family: document.family.clone(),
        model: document.model.clone(),
        source_hash,
        profile_hash,
        protocol,
        probes: probes.into_boxed_slice(),
        parameters,
        aliases,
        groups: groups.into_boxed_slice(),
        fault_source,
        faults,
        presets: presets.into_boxed_slice(),
        restore_order: restore_order.into_boxed_slice(),
        normalized_document: document,
    })
}
