use super::super::*;

pub(super) fn parse_decimal(value: &str, path: String) -> Result<Decimal, ProfileError> {
    Decimal::from_str(value).map_err(|error| ProfileError::validation(path, error))
}

pub(super) fn parse_non_negative_decimal(
    value: &str,
    path: String,
) -> Result<Decimal, ProfileError> {
    let parsed = parse_decimal(value, path.clone())?;
    if parsed.is_sign_negative() {
        return Err(ProfileError::validation(path, "value must be non-negative"));
    }
    Ok(parsed)
}

pub(super) fn canonical_decimal(value: Decimal) -> String {
    if value.is_zero() {
        "0".to_owned()
    } else {
        value.normalize().to_string()
    }
}
