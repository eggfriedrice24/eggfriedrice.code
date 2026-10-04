//! Where a screen sends what it produces while it processes bytes.

/// Receives the side effects of [`Screen::feed`](crate::Screen::feed) and
/// [`Screen::resize`](crate::Screen::resize).
///
/// Every call happens synchronously on the screen's thread, inside `feed` or
/// `resize`. The actor's sink buffers the calls and turns them into
/// [`ScreenEvent`](crate::ScreenEvent)s after each command, so a backend never blocks
/// on a channel inside a libghostty callback.
pub trait ScreenSink {
    /// Bytes to write back to the PTY: answers to DA, DSR, DECRQM and OSC 10 and 11
    /// queries. Only the daemon answers queries; clients never write replies.
    fn pty_reply(&mut self, bytes: &[u8]);

    /// The program rang the bell.
    fn bell(&mut self);

    /// The program set the window title (OSC 0 or OSC 2).
    fn title_changed(&mut self, title: &str);
}

/// One sink call other than a reply, kept in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    Bell,
    Title(String),
}

/// The actor's sink: it buffers replies and notices until the command finishes.
#[derive(Debug, Default)]
pub(crate) struct Collector {
    replies: Vec<u8>,
    notices: Vec<Notice>,
}

impl Collector {
    /// Takes the reply bytes buffered since the last call.
    pub(crate) fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Takes the notices buffered since the last call, oldest first.
    pub(crate) fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }
}

impl ScreenSink for Collector {
    fn pty_reply(&mut self, bytes: &[u8]) {
        self.replies.extend_from_slice(bytes);
    }

    fn bell(&mut self) {
        self.notices.push(Notice::Bell);
    }

    fn title_changed(&mut self, title: &str) {
        self.notices.push(Notice::Title(title.to_owned()));
    }
}

#[cfg(test)]
mod tests;
