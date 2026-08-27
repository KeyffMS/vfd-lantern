/// Presentation-only state of a text field.
///
/// Validation and domain interpretation belong to `lantern-app`; this type only
/// tracks what is currently visible and where the cursor is placed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FormState {
    value: String,
    cursor: usize,
}

impl FormState {
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn replace(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }
}
