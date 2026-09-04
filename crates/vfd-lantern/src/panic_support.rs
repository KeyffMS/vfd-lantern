use std::{panic, path::PathBuf, sync::Arc};

use lantern_storage::write_minimal_panic_report;
use lantern_tui::TerminalGuard;

pub fn install_terminal_panic_hook(guard: Arc<TerminalGuard>, panic_directory: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        let message = panic_message(information);
        run_panic_cleanup(
            || {
                let _ = guard.restore();
            },
            || {
                let _ = write_minimal_panic_report(&panic_directory, &message);
            },
            || previous(information),
        );
    }));
}

fn panic_message(information: &panic::PanicHookInfo<'_>) -> String {
    let payload = if let Some(message) = information.payload().downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = information.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    };
    match information.location() {
        Some(location) => format!(
            "{payload} at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => payload,
    }
}

fn run_panic_cleanup(restore: impl FnOnce(), report: impl FnOnce(), after_report: impl FnOnce()) {
    restore();
    report();
    after_report();
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::run_panic_cleanup;

    #[test]
    fn panic_cleanup_restores_terminal_before_report_and_follow_up_hook() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let restore_order = Rc::clone(&order);
        let report_order = Rc::clone(&order);
        let after_order = Rc::clone(&order);
        run_panic_cleanup(
            move || restore_order.borrow_mut().push("restore"),
            move || report_order.borrow_mut().push("report"),
            move || after_order.borrow_mut().push("after-report"),
        );
        assert_eq!(&*order.borrow(), &["restore", "report", "after-report"]);
    }
}
