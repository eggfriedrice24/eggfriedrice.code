//! The effect callbacks of a terminal and the buffer they fill.
//!
//! libghostty-vt runs every effect callback synchronously inside `vt_write` or
//! `resize`, on the screen thread. The callbacks are registered once, when the
//! terminal is built, so they cannot borrow the `ScreenSink` of the feed in progress.
//! They push into [`Effects`] instead, and the screen drains it into the sink after
//! each feed and each resize. That is the crate's own advice for `on_pty_write`, and
//! it means a callback never waits on anything.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use efr_screen::ScreenSink;
use libghostty_vt::Terminal;

/// What the callbacks of one terminal produced since the last drain.
///
/// Shared through an `Rc` between the terminal's callbacks and the screen, so the
/// callbacks are `'static` and the terminal borrows nothing. A `RefCell` borrow lasts
/// one statement: callbacks run only inside libghostty calls and the drain runs only
/// between them, so two borrows never overlap.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    /// Answers to terminal queries (DA, DSR, DECRQM, OSC 10 and 11, in-band resize
    /// reports), in the order libghostty-vt wrote them.
    replies: RefCell<Vec<u8>>,
    /// Bells and title changes, in the order they happened.
    notices: RefCell<Vec<Notice>>,
    /// Set when libghostty-vt changed its working directory, cleared by
    /// [`take_pwd_changed`](Effects::take_pwd_changed).
    pwd_changed: Cell<bool>,
}

/// A callback other than a reply, kept in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    Bell,
    Title(String),
}

impl Effects {
    /// Registers the callbacks on `terminal` and returns the buffer they fill.
    pub(crate) fn install(
        terminal: &mut Terminal<'static, 'static>,
    ) -> Result<Rc<Effects>, libghostty_vt::Error> {
        let effects = Rc::new(Effects::default());
        let replies = Rc::clone(&effects);
        let bells = Rc::clone(&effects);
        let titles = Rc::clone(&effects);
        let pwd = Rc::clone(&effects);
        terminal
            .on_pty_write(move |_, bytes| replies.replies.borrow_mut().extend_from_slice(bytes))?
            .on_bell(move |_| bells.notices.borrow_mut().push(Notice::Bell))?
            .on_title_changed(move |terminal| {
                // NOTE: a title that is not UTF-8 cannot be read through the safe API,
                // so it is not reported; title() then shows no title either.
                if let Ok(title) = terminal.title() {
                    titles.notices.borrow_mut().push(Notice::Title(title.to_owned()));
                }
            })?
            .on_pwd_changed(move |_| pwd.pwd_changed.set(true))?;
        Ok(effects)
    }

    /// Hands everything buffered since the last drain to `sink`: the notices in the
    /// order they happened, then the reply bytes in one call.
    pub(crate) fn drain(&self, sink: &mut dyn ScreenSink) {
        let notices = std::mem::take(&mut *self.notices.borrow_mut());
        for notice in notices {
            match notice {
                Notice::Bell => sink.bell(),
                Notice::Title(title) => sink.title_changed(&title),
            }
        }
        let replies = std::mem::take(&mut *self.replies.borrow_mut());
        if !replies.is_empty() {
            sink.pty_reply(&replies);
        }
    }

    /// True when libghostty-vt changed its working directory since the last call.
    pub(crate) fn take_pwd_changed(&self) -> bool {
        self.pwd_changed.replace(false)
    }
}

#[cfg(test)]
mod tests;
