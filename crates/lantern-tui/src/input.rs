use std::io;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use lantern_app::ApplicationView;

use crate::{MappedAction, UiAction, UiState, map_key};

pub struct InputReader {
    events: EventStream,
}

impl InputReader {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: EventStream::new(),
        }
    }

    pub async fn next_action(
        &mut self,
        ui: &UiState,
        view: &ApplicationView,
    ) -> io::Result<MappedAction> {
        loop {
            let event = self.events.next().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream closed")
            })??;
            match event {
                Event::Key(key) => {
                    if let Some(action) = map_key(ui, view, key) {
                        return Ok(action);
                    }
                }
                Event::Resize(width, height) => {
                    return Ok(MappedAction::Ui(UiAction::Resize { width, height }));
                }
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) | Event::Paste(_) => {}
            }
        }
    }
}

impl Default for InputReader {
    fn default() -> Self {
        Self::new()
    }
}
