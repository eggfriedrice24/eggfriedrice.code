use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};

use pretty_assertions::assert_eq;

use super::*;

/// The panic hook is global: the tests that change it take turns, also under a test
/// runner that runs them on threads of one process.
static HOOK: Mutex<()> = Mutex::new(());

#[test]
fn quiet_holds_the_flag_until_it_ends_also_on_a_panic() {
    let _hook = HOOK.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(!is_quiet());
    assert!(quiet(is_quiet));
    assert!(quiet(|| quiet(is_quiet)), "a quiet call inside another one");
    assert!(!is_quiet());
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let unwound = panic::catch_unwind(|| quiet(|| panic!("a secret")));
    panic::set_hook(previous);
    assert!(unwound.is_err());
    assert!(!is_quiet(), "the flag goes down as the panic unwinds");
}

#[test]
fn quiet_report_names_the_place_but_not_the_message() {
    let _hook = HOOK.lock().unwrap_or_else(PoisonError::into_inner);
    let seen: Arc<Mutex<Vec<(bool, String)>>> = Arc::default();
    let previous = panic::take_hook();
    let sink = Arc::clone(&seen);
    panic::set_hook(Box::new(move |info| {
        let report = quiet_report(info);
        sink.lock().unwrap_or_else(PoisonError::into_inner).push((is_quiet(), report));
    }));
    let text = String::from("password=hunter2");
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        quiet(|| {
            let at = text.find('=').unwrap_or(0) + 100;
            text[..at].len()
        })
    }));
    panic::set_hook(previous);
    assert!(unwound.is_err());
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(seen.len(), 1);
    let (was_quiet, report) = &seen[0];
    assert!(was_quiet, "the hook runs before the panic unwinds");
    assert!(report.contains("panic/tests.rs"), "{report}");
    let name = std::thread::current().name().unwrap_or("<unnamed>").to_owned();
    assert!(report.starts_with(&format!("thread '{name}' panicked at ")), "{report}");
    assert!(!report.contains("hunter2"), "{report}");
}
