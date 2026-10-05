//! `Ctrl+\` as the user's way to type an input for a command that prints nothing.
//!
//! While a shell call runs silently and reports no wait, `efr` must not read keys: text
//! typed then goes on to the user's shell as typeahead, which the user chose. So the
//! view only says that `Ctrl+\` opens an answer line, and the terminal turns that key into
//! SIGQUIT for `efr`, the foreground job, without `efr` reading anything.
//!
//! The key counts only while a [`Quit::wait`] future lives, which the follow loop keeps
//! exactly while that line is shown. Any other SIGQUIT keeps its default meaning: the
//! process ends as it would without a handler. tokio installs a handler once and never
//! removes it, so the listener task, started at the first wait, takes every SIGQUIT and
//! does the default action itself for one that comes while nothing waits.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

use crate::context::Stop;

/// Where `Ctrl+\` comes from.
pub(crate) trait Quit: Send + Sync + fmt::Debug {
    /// Resolves at the next `Ctrl+\` while the returned future lives. While no such future
    /// lives, `Ctrl+\` keeps its default meaning.
    fn wait(&self) -> Stop;
}

/// `Ctrl+\` from the terminal, as SIGQUIT.
#[derive(Debug, Clone)]
pub(crate) struct CtrlBackslash {
    state: Arc<State>,
}

struct State {
    /// How many [`Quit::wait`] futures live.
    armed: AtomicUsize,
    /// Counts the presses that came while a future lived.
    presses: watch::Sender<u64>,
    /// Starts the listener once.
    listening: Once,
    /// What a SIGQUIT does while nothing waits: the default action, or a test's stand-in.
    unarmed: fn(),
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("armed", &self.armed.load(Ordering::Relaxed))
            .field("presses", &*self.presses.borrow())
            .finish_non_exhaustive()
    }
}

impl CtrlBackslash {
    /// `Ctrl+\` whose SIGQUIT ends the process as usual while nothing waits for it.
    pub(crate) fn new() -> Self {
        CtrlBackslash::with_unarmed(default_action)
    }

    /// `Ctrl+\` that calls `unarmed` for a SIGQUIT while nothing waits for it.
    pub(crate) fn with_unarmed(unarmed: fn()) -> Self {
        let (presses, _) = watch::channel(0);
        CtrlBackslash {
            state: Arc::new(State {
                armed: AtomicUsize::new(0),
                presses,
                listening: Once::new(),
                unarmed,
            }),
        }
    }
}

impl Quit for CtrlBackslash {
    fn wait(&self) -> Stop {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            // Subscribed before the count goes up, so only a press from now on counts.
            let mut presses = state.presses.subscribe();
            let _armed = Armed::new(&state);
            listen(&state);
            if presses.changed().await.is_err() {
                // The sender lives in the state, which this future holds.
                std::future::pending::<()>().await;
            }
        })
    }
}

/// One live wait, counted while it lives.
struct Armed<'a>(&'a State);

impl<'a> Armed<'a> {
    fn new(state: &'a State) -> Self {
        state.armed.fetch_add(1, Ordering::AcqRel);
        Armed(state)
    }
}

impl Drop for Armed<'_> {
    fn drop(&mut self) {
        self.0.armed.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Starts the task that takes every SIGQUIT, once per state. When the handler cannot be
/// installed, nothing waits for the key, and SIGQUIT keeps its default action anyway.
fn listen(state: &Arc<State>) {
    state.listening.call_once(|| match signal(SignalKind::quit()) {
        Ok(mut quits) => {
            let state = Arc::clone(state);
            tokio::spawn(async move {
                while quits.recv().await.is_some() {
                    if state.armed.load(Ordering::Acquire) > 0 {
                        state.presses.send_modify(|count| *count = count.wrapping_add(1));
                    } else {
                        (state.unarmed)();
                    }
                }
            });
        }
        Err(error) => tracing::debug!(%error, "the Ctrl+\\ handler could not be installed"),
    });
}

/// What SIGQUIT does without a handler: the process ends, with a core dump where the
/// system keeps them.
fn default_action() {
    // NOTE: this puts the default disposition back and raises the signal again, so the
    // shell sees `efr` end by SIGQUIT, as it would without the handler.
    if let Err(error) =
        signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGQUIT)
    {
        tracing::warn!(%error, "SIGQUIT could not take its default action");
    }
}

#[cfg(test)]
mod tests;
