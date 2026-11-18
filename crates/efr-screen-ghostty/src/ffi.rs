//! The one `unsafe` call of the crate: the log hook of libghostty-vt.
//!
//! libghostty-vt sends its log to one callback for the whole process. When no callback
//! is set, it drops a log line before it formats it. `libghostty_vt::log::set_logger`
//! sets that callback. It is `unsafe` because libghostty-vt does not lock the
//! setting: no other thread may be inside libghostty-vt while it changes. [`init`]
//! sets it once, before the first terminal of the process exists, and every path into
//! libghostty-vt in this crate calls [`init`] first.
//!
//! The log goes to `tracing` with the target [`TARGET`], at `debug` for the errors
//! and warnings of the library and at `trace` for the rest. Not higher: the program
//! in the hidden shell controls what the library logs (a `CSI t` without parameters
//! is an error there, an unknown erase mode or a bad cursor move is a warning), so a
//! higher level would let any program fill the journal. A real failure inside the
//! library shows as the `vt_processing_error` flag, which the screen logs as a
//! warning once.
//!
//! [`init`] sets the hook only when the log filter lets `debug` events of [`TARGET`]
//! through. Without the hook, the library formats no log line that `tracing` would
//! drop. The filter is read once, when the first screen of the process starts: a
//! later change of the filter does not set or clear the hook.

use std::panic;
use std::sync::Once;

use libghostty_vt::log::{Level, Logger};

/// The `tracing` target of the library log. `EFR_LOG=info,libghostty=debug` shows it.
pub(crate) const TARGET: &str = "libghostty";

static INIT: Once = Once::new();

/// Sets the log hook of libghostty-vt, once for the process, when the log filter
/// shows the library log. Call it before any other use of libghostty-vt.
pub(crate) fn init() {
    INIT.call_once(|| {
        if !tracing::enabled!(target: TARGET, tracing::Level::DEBUG) {
            return;
        }
        // SAFETY: `set_logger` must not run while another thread is inside
        // libghostty-vt. Only this crate links libghostty-vt (`cargo xtask deps`), and
        // in this crate every path to a libghostty-vt call goes through `init` first:
        // `GhosttyScreen::new`, `snapshot::decode` (the only way to a restored
        // terminal) and the test helper that builds a bare terminal. `Once::call_once`
        // runs this closure on the first caller and blocks every other caller until it
        // returns. So no thread has made a libghostty-vt call yet while it runs, and
        // none can start one.
        let set = unsafe { libghostty_vt::log::set_logger(Some(Box::new(Tracing))) };
        if let Err(error) = set {
            tracing::warn!(%error, "the log of libghostty-vt is lost: its log hook was refused");
        }
    });
}

/// The bridge from the library log to `tracing`.
///
/// libghostty-vt calls it from C through the binding's `extern "C"` callback, where an
/// unwinding panic aborts efrd, so [`Logger::log`] catches every panic of the
/// subscriber and drops that line. The binding holds a read lock on the hook while
/// this runs, so it must not make libghostty-vt log and must not call `set_logger`: it
/// only hands the line to `tracing`, and no subscriber of efr calls libghostty-vt.
struct Tracing;

impl Logger for Tracing {
    fn log(&self, level: Level, scope: &str, message: &str) {
        let _ = panic::catch_unwind(|| match level {
            Level::Error | Level::Warning => {
                tracing::debug!(target: TARGET, scope, ghostty_level = ?level, "{message}");
            }
            _ => tracing::trace!(target: TARGET, scope, ghostty_level = ?level, "{message}"),
        });
    }
}

#[cfg(test)]
mod tests;
