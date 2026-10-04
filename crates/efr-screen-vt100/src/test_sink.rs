//! A sink for this crate's tests that keeps every call in order.

use efr_screen::ScreenSink;

/// One call a screen made on its sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    Reply(Vec<u8>),
    Bell,
    Title(String),
}

#[derive(Debug, Default)]
pub(crate) struct TestSink {
    pub(crate) calls: Vec<Call>,
}

impl TestSink {
    pub(crate) fn take(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.calls)
    }
}

impl ScreenSink for TestSink {
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
