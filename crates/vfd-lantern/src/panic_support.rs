use std::{panic, sync::Arc};

use lantern_tui::TerminalGuard;

pub fn install_terminal_panic_hook(guard: Arc<TerminalGuard>) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        run_panic_cleanup(
            || {
                let _ = guard.restore();
            },
            || previous(information),
        );
    }));
}

fn run_panic_cleanup(restore: impl FnOnce(), after_restore: impl FnOnce()) {
    restore();
    after_restore();
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::run_panic_cleanup;

    #[test]
    fn panic_cleanup_restores_terminal_before_follow_up_reporting_hook() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let restore_order = Rc::clone(&order);
        let report_order = Rc::clone(&order);
        run_panic_cleanup(
            move || restore_order.borrow_mut().push("restore"),
            move || report_order.borrow_mut().push("after-restore"),
        );
        assert_eq!(&*order.borrow(), &["restore", "after-restore"]);
    }
}
