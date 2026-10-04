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
