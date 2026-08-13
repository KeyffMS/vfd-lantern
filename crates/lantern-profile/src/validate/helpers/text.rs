use super::super::*;

pub(super) fn validate_text(
    path: impl Into<String>,
    value: &str,
    allow_empty: bool,
) -> Result<(), ProfileError> {
    let path = path.into();
    if !allow_empty && value.is_empty() {
        return Err(ProfileError::validation(path, "text must not be empty"));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(ProfileError::validation(
            path,
            format!("text exceeds {MAX_TEXT_BYTES} bytes"),
        ));
    }
    if let Some((index, character)) = value
        .char_indices()
        .find(|(_, character)| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(ProfileError::validation(
            path,
            format!("contains control character {character:?} at byte {index}"),
        ));
    }
    Ok(())
}
