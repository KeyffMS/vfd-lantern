use std::{
    io::{self, Stdout, stdout},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{ApplicationView, InputReader, MappedAction, Theme, UiState, render};

trait TerminalControl: Send + Sync {
    fn restore(&self) -> io::Result<()>;
}

struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn restore(&self) -> io::Result<()> {
        let raw_result = disable_raw_mode();
        let screen_result = execute!(stdout(), Show, LeaveAlternateScreen);
        raw_result.and(screen_result)
    }
}

/// Sole owner of raw-mode/alternate-screen/cursor lifecycle.
///
/// `restore()` is idempotent and may safely be called from the normal shutdown
/// path, an error path, a signal path, the composition-root panic hook and Drop.
pub struct TerminalGuard {
    active: AtomicBool,
    control: Arc<dyn TerminalControl>,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            active: AtomicBool::new(true),
            control: Arc::new(CrosstermControl),
        })
    }

    pub fn restore(&self) -> io::Result<()> {
        if !self.active.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        self.control.restore()
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn with_control(control: Arc<dyn TerminalControl>) -> Self {
        Self {
            active: AtomicBool::new(true),
            control,
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub struct TerminalSession {
    guard: Arc<TerminalGuard>,
    terminal: Terminal<CrosstermBackend<Stdout>>,
    input: InputReader,
    theme: Theme,
}

impl TerminalSession {
    pub fn enter(color_enabled: bool) -> io::Result<Self> {
        let guard = Arc::new(TerminalGuard::enter()?);
        let backend = CrosstermBackend::new(stdout());
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = guard.restore();
                return Err(error);
            }
        };
        if let Err(error) = terminal.clear() {
            let _ = guard.restore();
            return Err(error);
        }
        Ok(Self {
            guard,
            terminal,
            input: InputReader::new(),
            theme: Theme::new(color_enabled),
        })
    }

    #[must_use]
    pub fn guard(&self) -> Arc<TerminalGuard> {
        Arc::clone(&self.guard)
    }

    pub fn initialize_viewport(&self, ui: &mut UiState) -> io::Result<()> {
        let area = self.terminal.size()?;
        ui.apply(crate::UiAction::Resize {
            width: area.width,
            height: area.height,
        });
        Ok(())
    }

    pub async fn next_action(&mut self, ui: &UiState) -> io::Result<MappedAction> {
        self.input.next_action(ui).await
    }

    pub fn draw(&mut self, view: &ApplicationView, ui: &UiState) -> io::Result<()> {
        let theme = self.theme;
        self.terminal.draw(|frame| render(frame, view, ui, theme))?;
        Ok(())
    }

    pub fn restore(&self) -> io::Result<()> {
        self.guard.restore()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.guard.restore();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::{TerminalControl, TerminalGuard};

    struct RecordingControl {
        calls: Arc<AtomicUsize>,
        fail: bool,
    }

    impl TerminalControl for RecordingControl {
        fn restore(&self) -> io::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(io::Error::other("synthetic restore failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn restore_is_idempotent_on_success() {
        let calls = Arc::new(AtomicUsize::new(0));
        let guard = TerminalGuard::with_control(Arc::new(RecordingControl {
            calls: Arc::clone(&calls),
            fail: false,
        }));
        guard.restore().expect("first restore");
        guard.restore().expect("second restore");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!guard.is_active());
    }

    #[test]
    fn restore_failure_does_not_reactivate_terminal() {
        let calls = Arc::new(AtomicUsize::new(0));
        let guard = TerminalGuard::with_control(Arc::new(RecordingControl {
            calls: Arc::clone(&calls),
            fail: true,
        }));
        assert!(guard.restore().is_err());
        guard.restore().expect("idempotent second restore");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!guard.is_active());
    }
}
