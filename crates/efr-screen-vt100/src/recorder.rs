//! What vt100 reports through its callbacks while it parses: bells, titles and the
//! OSC 7 working directory.
//!
//! vt100 owns its `Callbacks` value for the life of the parser, while a
//! [`ScreenSink`] is lent to each [`Screen::feed`](efr_screen::Screen::feed) call
//! only. So the recorder keeps what happened during one `process` call in order, and
//! the screen hands it to that call's sink right after.

use efr_screen::ScreenSink;

/// The longest title kept, in bytes. libghostty-vt cuts a title at the same length,
/// so a hostile title costs both backends the same memory.
pub(crate) const MAX_TITLE_BYTES: usize = 1024;

/// The longest OSC 7 URL kept, in bytes: Linux `PATH_MAX` plus room for the scheme,
/// the host and percent-encoding, the limit libghostty-vt uses.
pub(crate) const MAX_PWD_BYTES: usize = 4096;

/// One callback that the sink must hear about, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    Bell,
    Title(String),
}

/// The `vt100::Callbacks` of a [`Vt100Screen`](crate::Vt100Screen).
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    title: Option<String>,
    pwd: Option<String>,
    notices: Vec<Notice>,
}

impl Recorder {
    /// The title the program set last; `None` before any title and after an empty
    /// one, as in libghostty-vt.
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The URL of the last OSC 7 report exactly as the program sent it
    /// (`file://host/path` or `kitty-shell-cwd://host/path`), the same raw form
    /// libghostty-vt's `Terminal::pwd` returns, so the cross-check against the
    /// scanner's marks reads both backends alike. `None` before any report and after
    /// an empty one.
    pub(crate) fn pwd(&self) -> Option<&str> {
        self.pwd.as_deref()
    }

    /// Hands the notices recorded since the last call to `sink`, oldest first.
    pub(crate) fn drain_into(&mut self, sink: &mut dyn ScreenSink) {
        for notice in self.notices.drain(..) {
            match notice {
                Notice::Bell => sink.bell(),
                Notice::Title(title) => sink.title_changed(&title),
            }
        }
    }

    fn set_title(&mut self, raw: &[u8]) {
        let title = capped(raw, MAX_TITLE_BYTES);
        self.title = (!title.is_empty()).then(|| title.clone());
        self.notices.push(Notice::Title(title));
    }

    fn set_pwd(&mut self, raw: &[u8]) {
        let url = capped(raw, MAX_PWD_BYTES);
        self.pwd = (!url.is_empty()).then_some(url);
    }
}

impl vt100::Callbacks for Recorder {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.notices.push(Notice::Bell);
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.set_title(title);
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            // vte splits an OSC body at every `;`, so vt100 matches OSC 0 and 2 only
            // when the title holds none; a title with one arrives here in pieces.
            [b"0" | b"2", rest @ ..] if rest.len() > 1 => self.set_title(&rest.join(&b';')),
            [b"7", rest @ ..] if !rest.is_empty() => self.set_pwd(&rest.join(&b';')),
            _ => {}
        }
    }
}

/// The bytes as UTF-8 (invalid sequences replaced), cut to at most `limit` bytes on a
/// character boundary.
fn capped(raw: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(raw);
    let end = text.floor_char_boundary(limit);
    text[..end].to_owned()
}

#[cfg(test)]
mod tests;
