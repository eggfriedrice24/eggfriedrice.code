//! The launcher's job-control signals (the spec's section 3.13).
//!
//! Ctrl+C, Ctrl+\ and Ctrl+Z from the hidden PTY, and efr's own interrupt, go to the
//! whole foreground job: the launcher, bwrap and every sandboxed process. The launcher
//! must outlive them to write `result.json`, so it catches SIGINT, SIGQUIT and SIGTSTP
//! with a handler that only sets a flag. It never ignores them: an ignored signal stays
//! ignored across `fork` and `exec`, and bwrap and the child would ignore Ctrl+C too. A
//! caught signal goes back to its default action at `exec`, so every child gets the
//! default.
//!
//! The flags of SIGINT and SIGQUIT tell the launcher that the user stopped the call.
//! A signal that comes before bwrap or the exit child starts reaches only the
//! launcher, so the launcher does not start the call, and a setup that the signal
//! broke is reported as an interrupt (status 130), not as a setup failure (125).
//!
//! A job (`efr-sbx run --job`) also catches SIGTERM and SIGHUP, which stop it.

use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGTSTP};

use crate::error::SbxError;
use crate::job::Event;

/// The signals the launcher catches.
pub(crate) const CAUGHT: [i32; 3] = [SIGINT, SIGQUIT, SIGTSTP];

/// The signals that stop a call, in the order in which [`interrupted`] names them.
const STOPS: [i32; 2] = [SIGINT, SIGQUIT];

/// One flag per signal of [`CAUGHT`], set by its handler.
fn flags() -> &'static [Arc<AtomicBool>; 3] {
    static FLAGS: OnceLock<[Arc<AtomicBool>; 3]> = OnceLock::new();
    FLAGS.get_or_init(Default::default)
}

/// Installs the handlers. They stay for the life of the process.
pub(crate) fn catch_job_control() -> Result<(), SbxError> {
    for (signal, flag) in CAUGHT.into_iter().zip(flags()) {
        signal_hook::flag::register(signal, Arc::clone(flag))
            .map_err(|error| SbxError::os("catch the job-control signals", error))?;
    }
    Ok(())
}

/// The signal that stopped the call, when SIGINT or SIGQUIT came since the handlers
/// were installed.
pub(crate) fn interrupted() -> Option<i32> {
    STOPS.into_iter().find(|signal| {
        CAUGHT
            .iter()
            .zip(flags())
            .any(|(caught, flag)| caught == signal && flag.load(Ordering::SeqCst))
    })
}

/// The signals that stop a job: efrd sends SIGTERM, and SIGHUP comes from a `kill`
/// only, since a job has no terminal.
const JOB_STOPS: [i32; 2] = [SIGTERM, SIGHUP];

/// In job mode, catches SIGTERM and SIGHUP: each one sends [`Event::Signal`] to the
/// job's stop thread. The handler writes a byte on a pipe and a thread of its own
/// reads it; the signals are caught, never ignored, so every child gets the default
/// action again at `exec`.
pub(crate) fn catch_stop(events: Sender<Event>) -> Result<(), SbxError> {
    let failed = |error| SbxError::os("catch SIGTERM and SIGHUP", error);
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|error| failed(error.into()))?;
    for signal in JOB_STOPS {
        let end = write.try_clone().map_err(failed)?;
        signal_hook::low_level::pipe::register(signal, end).map_err(failed)?;
    }
    drop(write);
    std::thread::Builder::new()
        .name("efr-sbx-signals".to_owned())
        .spawn(move || {
            let mut read = File::from(read);
            let mut bytes = [0_u8; 16];
            loop {
                match read.read(&mut bytes) {
                    Ok(0) => return,
                    Ok(_) => {
                        if events.send(Event::Signal).is_err() {
                            return;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return,
                }
            }
        })
        .map(drop)
        .map_err(failed)
}

/// Sends the signal that stopped the call to the launcher's own process group, the
/// job of the call, once the launcher started bwrap or the exit child. A signal that
/// came while the child was being started reached only the launcher.
///
/// NOTE: the group and not the child alone: bwrap clones the namespace's init and only
/// then sets its parent-death signal. A signal to bwrap alone in that window leaves the
/// init behind, which holds the launcher's pipes open forever. A signal to the group
/// reaches a clone that is under way too. The launcher catches it itself.
pub(crate) fn forward_interrupt() {
    let Some(signal) = interrupted().and_then(rustix::process::Signal::from_named_raw) else {
        return;
    };
    // NOTE: a failure leaves the call to run; the status still reports the signal.
    let _ = rustix::process::kill_current_process_group(signal);
}
