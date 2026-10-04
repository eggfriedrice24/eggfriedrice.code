//! Helpers for this crate's unit tests.

use std::rc::Rc;

use efr_screen::{RowCells, ScreenSink, row_text};
use libghostty_vt::Terminal;

use crate::effects::Effects;

/// One call a screen made on its sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    Reply(Vec<u8>),
    Bell,
    Title(String),
}

/// A sink that writes down every call in order.
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    pub(crate) calls: Vec<Call>,
}

impl Recorder {
    /// Every reply byte, in order.
    pub(crate) fn replies(&self) -> Vec<u8> {
        self.calls
            .iter()
            .filter_map(|call| match call {
                Call::Reply(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect()
    }
}

impl ScreenSink for Recorder {
    fn pty_reply(&mut self, bytes: &[u8]) {
        self.calls.push(Call::Reply(bytes.to_vec()));
    }

    fn bell(&mut self) {
        self.calls.push(Call::Bell);
    }

    fn title_changed(&mut self, title: &str) {
        self.calls.push(Call::Title(title.to_owned()));
    }
}

/// A bare terminal with the crate's effect callbacks installed.
pub(crate) fn terminal(cols: u16, rows: u16) -> (Terminal<'static, 'static>, Rc<Effects>) {
    let mut terminal = Terminal::new(cols, rows).unwrap();
    let effects = Effects::install(&mut terminal).unwrap();
    (terminal, effects)
}

/// The text of each row, as the conformance suite compares it.
pub(crate) fn texts(rows: &[RowCells]) -> Vec<String> {
    rows.iter().map(row_text).collect()
}
