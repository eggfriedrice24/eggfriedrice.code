//! The workspace's own crates keep line tables in dev and test builds (the root
//! `Cargo.toml`, `[profile.dev]`), so a panic's backtrace still names the file and the
//! line where it happened, although the rest of the debug info is gone.

use std::backtrace::Backtrace;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

#[test]
fn a_panics_backtrace_names_the_file_and_the_line() {
    let captured = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&captured);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |_| {
        let trace = Backtrace::force_capture().to_string();
        *sink.lock().unwrap_or_else(PoisonError::into_inner) = trace;
    }));
    let line = line!() + 1;
    let outcome = std::panic::catch_unwind(|| panic!("a panic on purpose"));
    std::panic::set_hook(previous);

    assert!(outcome.is_err());
    let trace = captured.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // The trace names the file relative to the directory the test runs in, which is not
    // the one `file!()` is relative to, so only the file name is compared.
    let file = Path::new(file!()).file_name().and_then(|name| name.to_str()).unwrap();
    let place = format!("{file}:{line}:");
    assert!(trace.contains(&place), "no {place} in the backtrace:\n{trace}");
}
