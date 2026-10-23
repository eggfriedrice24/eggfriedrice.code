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
//!
//! The launcher blocks each signal while it installs its handler (see [`blocked`]). A
//! signal before the install ends the launcher, and a signal during or after the
//! install sets its flag. No signal is lost.

use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use nix::sys::signal::{SigSet, SigmaskHow, Signal};
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
    let caught = CAUGHT.into_iter().zip(flags().iter().map(Arc::clone));
    catch_flags(caught, |_| {})
}

/// Installs a handler that sets `flag` for each pair of `caught`, with the signals
/// blocked. `before` runs right before each install, for the tests.
fn catch_flags(
    caught: impl IntoIterator<Item = (i32, Arc<AtomicBool>)>,
    mut before: impl FnMut(i32),
) -> Result<(), SbxError> {
    let caught: Vec<_> = caught.into_iter().collect();
    let signals: Vec<i32> = caught.iter().map(|(signal, _)| *signal).collect();
    blocked(&signals, "catch the job-control signals", || {
        for (signal, flag) in caught {
            before(signal);
            signal_hook::flag::register(signal, flag)
                .map_err(|error| SbxError::os("catch the job-control signals", error))?;
        }
        Ok(())
    })
}

/// Runs `install` with `signals` blocked on the calling thread, and then puts the
/// thread's mask back.
///
/// NOTE: signal-hook installs its handler with `sigaction` first, and only after that
/// it stores the action that the handler runs. A signal in between runs a handler that
/// finds no action, and the signal is lost: it does not end the process, which no
/// longer has the default action, and it sets no flag. A Ctrl+C at that moment let
/// the call run to its end. While the signal is blocked, it stays pending, and the
/// kernel delivers it when the mask is put back, to the complete handler. The callers
/// run on the main thread while it is the only thread, so a signal to the process
/// waits too, and no thread starts with the signals blocked.
fn blocked(
    signals: &[i32],
    what: &'static str,
    install: impl FnOnce() -> Result<(), SbxError>,
) -> Result<(), SbxError> {
    let failed = |error: nix::errno::Errno| SbxError::os(what, error.into());
    let mut held = SigSet::empty();
    for &signal in signals {
        held.add(Signal::try_from(signal).map_err(failed)?);
    }
    let before = held.thread_swap_mask(SigmaskHow::SIG_BLOCK).map_err(failed)?;
    let installed = install();
    before.thread_set_mask().map_err(failed)?;
    installed
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
    blocked(&JOB_STOPS, "catch SIGTERM and SIGHUP", || {
        for signal in JOB_STOPS {
            let end = write.try_clone().map_err(failed)?;
            signal_hook::low_level::pipe::register(signal, end).map_err(failed)?;
        }
        Ok(())
    })?;
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

#[cfg(test)]
mod tests;
