//! Panics in code that holds private text.
//!
//! The message of a panic can quote the text it failed on: a slice panic quotes up to
//! 256 bytes of its string, and Rust's default hook writes the message to stderr,
//! which for efrd goes to the journal. Code that works on private text, such as a
//! secret before its redaction or a search query, runs inside [`quiet`]. The hook of
//! [`install_quiet_hook`] then writes only where the panic happened and the thread's
//! name, never the message. Outside [`quiet`] it reports as before.

use std::cell::Cell;
use std::io::Write as _;
use std::panic::{self, PanicHookInfo};

thread_local! {
    /// How many [`quiet`] calls run on this thread.
    static QUIET: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` with the panic message hidden on this thread: a panic inside it reports
/// only its place and the thread. A panic still unwinds out of `f` as before.
///
/// ```
/// let sum = efr_stdx::panic::quiet(|| 2 + 2);
/// assert_eq!(sum, 4);
/// assert!(!efr_stdx::panic::is_quiet());
/// ```
pub fn quiet<T>(f: impl FnOnce() -> T) -> T {
    let _quiet = Quiet::enter();
    f()
}

/// True while a [`quiet`] call runs on this thread.
pub fn is_quiet() -> bool {
    QUIET.with(Cell::get) > 0
}

/// Keeps the flag of this thread up until it is dropped, also while a panic unwinds.
struct Quiet;

impl Quiet {
    fn enter() -> Self {
        QUIET.with(|quiet| quiet.set(quiet.get().saturating_add(1)));
        Quiet
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.with(|quiet| quiet.set(quiet.get().saturating_sub(1)));
    }
}

/// Puts a hook before the current panic hook: inside [`quiet`] it writes one line with
/// the thread's name and the place of the panic to stderr, and no message; elsewhere
/// it runs the hook that was there. A daemon calls it once at its start.
pub fn install_quiet_hook() {
    let report = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if is_quiet() {
            let _ = std::io::stderr().write_all(quiet_report(info).as_bytes());
        } else {
            report(info);
        }
    }));
}

/// The line of a panic inside [`quiet`]: the thread and the place, not the message.
fn quiet_report(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    let place = info
        .location()
        .map_or_else(String::new, |at| format!(" at {}:{}:{}", at.file(), at.line(), at.column()));
    format!("thread '{name}' panicked{place}; the message is hidden, it can hold private text\n")
}

#[cfg(test)]
mod tests;
