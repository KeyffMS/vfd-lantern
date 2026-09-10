#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackupUiState {
    pub confirmation_active: bool,
    pub confirmation_input: String,
}

impl BackupUiState {
    pub fn begin_confirmation(&mut self) {
        self.confirmation_active = true;
        self.confirmation_input.clear();
    }

    pub fn cancel_confirmation(&mut self) {
        self.confirmation_active = false;
        self.confirmation_input.clear();
    }

    pub fn insert(&mut self, character: char) {
        if self.confirmation_active {
            self.confirmation_input.push(character);
        }
    }

    pub fn backspace(&mut self) {
        if self.confirmation_active {
            self.confirmation_input.pop();
        }
    }
}
