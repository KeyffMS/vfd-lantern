use super::super::{helpers::*, *};
use super::presets::validate_parameter_references;

pub(super) fn validate_aliases(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<BTreeMap<String, ParameterId>, ProfileError> {
    let mut aliases = BTreeMap::new();
    for (alias, target) in &document.aliases {
        let alias_id = ParameterId::parse(alias.clone())
            .map_err(|error| ProfileError::validation(format!("aliases.{alias}"), error))?;
        let target_id = ParameterId::parse(target.clone())
            .map_err(|error| ProfileError::validation(format!("aliases.{alias}"), error))?;
        if !parameters.contains_key(&target_id) {
            return Err(ProfileError::validation(
                format!("aliases.{alias}"),
                format!("unknown parameter {target_id}"),
            ));
        }
        aliases.insert(alias_id.as_str().to_owned(), target_id);
    }
    Ok(aliases)
}

pub(super) fn validate_groups(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedParameterGroup>, ProfileError> {
    let mut group_ids = BTreeSet::new();
    document
        .groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let base = format!("groups[{index}]");
            validate_text(format!("{base}.id"), &group.id, false)?;
            validate_text(format!("{base}.name"), &group.name, false)?;
            if !group_ids.insert(group.id.clone()) {
                return Err(ProfileError::validation(
                    format!("{base}.id"),
                    "duplicate group ID",
                ));
            }
            let parameters = validate_parameter_references(
                &group.parameters,
                parameters,
                format!("{base}.parameters"),
            )?;
            Ok(ValidatedParameterGroup {
                id: group.id.clone(),
                name: group.name.clone(),
                parameters: parameters.into_boxed_slice(),
            })
        })
        .collect()
}
