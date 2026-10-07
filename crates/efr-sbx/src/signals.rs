//! The launcher's job-control signals (the spec's section 3.13).
//!
//! Ctrl+C, Ctrl+\ and Ctrl+Z from the hidden PTY, and efr's own interrupt, go to the
//! whole foreground job: the launcher, bwrap and every sandboxed process. The launcher
//! must outlive them to write `result.json`, so it catches SIGINT, SIGQUIT and SIGTSTP
//! with a handler that only sets a flag. It never ignores them: an ignored signal stays
//! ignored across `fork` and `exec`, and bwrap and the child would ignore Ctrl+C too. A
//! caught signal goes back to its default action at `exec`, so every child gets the
//! default.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use signal_hook::consts::{SIGINT, SIGQUIT, SIGTSTP};

use crate::error::SbxError;

/// The signals the launcher catches.
pub(crate) const CAUGHT: [i32; 3] = [SIGINT, SIGQUIT, SIGTSTP];

/// Installs the handlers. They stay for the life of the process; the flag they set is
/// never read, because the launcher only needs to survive the signal.
pub(crate) fn catch_job_control() -> Result<(), SbxError> {
    let caught = Arc::new(AtomicBool::new(false));
    for signal in CAUGHT {
        signal_hook::flag::register(signal, Arc::clone(&caught))
            .map_err(|error| SbxError::os("catch the job-control signals", error))?;
    }
    Ok(())
}
