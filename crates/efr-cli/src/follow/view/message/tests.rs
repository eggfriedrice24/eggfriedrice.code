use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_render::{RenderOptions, render};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};

use super::Message;
use crate::testing::now;

fn at(millis: i64) -> Timestamp {
    now() + SignedDuration::from_millis(millis)
}

const FRAME: Option<Duration> = Some(Duration::from_millis(16));

fn message() -> Message {
    Message::new(0, RenderOptions::new(40))
}

#[test]
fn the_first_text_shows_at_once() {
    let mut message = message();
    message.receive("Hello there.\n\n");
    message.reveal(at(0), None);
    assert!(!message.waiting());
}

#[test]
fn a_burst_after_a_pause_is_shown_over_the_time_until_the_next_one() {
    let mut message = message();
    message.receive("a");
    message.reveal(at(0), None);
    // The next burst comes 200 ms later, as persisted updates do: it shows over 120 ms.
    message.receive(&format!("a{}", "b".repeat(120)));
    message.reveal(at(200), FRAME);
    let shown = message.pushed;
    assert!(shown > 1 && shown < 121, "{shown}");
    message.reveal(at(216), FRAME);
    assert!(message.pushed > shown, "the text moves on at every frame");
    // The oldest text never waits longer than the window.
    message.reveal(at(320), FRAME);
    assert!(!message.waiting());
}

#[test]
fn drafts_a_frame_apart_show_within_a_frame() {
    let mut message = message();
    message.receive("one ");
    message.reveal(at(0), None);
    message.receive("one two ");
    message.reveal(at(16), FRAME);
    assert!(!message.waiting(), "{}", message.pushed);
}

#[test]
fn eight_waiting_lines_show_at_once() {
    let mut message = message();
    message.receive("x");
    message.reveal(at(0), None);
    message.receive(&format!("x{}", "line\n".repeat(8)));
    message.reveal(at(200), FRAME);
    assert!(!message.waiting());
}

#[test]
fn a_repeat_of_the_start_changes_nothing() {
    let mut message = message();
    message.receive("drafts ran ahead");
    message.receive("drafts ran");
    assert_eq!(message.received(), "drafts ran ahead");
    message.receive("drafts ran ahead of the update");
    assert_eq!(message.received(), "drafts ran ahead of the update");
}

/// What `f` logs at debug level and above.
fn logs(f: impl FnOnce()) -> String {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buffer);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || Sink(Arc::clone(&sink)))
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buffer.lock().unwrap().clone()).unwrap()
}

struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_update_behind_the_drafts_logs_nothing_and_a_rewrite_does() {
    let mut message = message();
    message.receive("drafts ran ahead");
    assert_eq!(logs(|| message.receive("drafts ran")), "");
    let rewrite = logs(|| message.receive("something else"));
    assert!(rewrite.contains("rewrote sent text"), "{rewrite}");
    assert_eq!(message.received(), "drafts ran ahead");
}

#[test]
fn a_cut_never_splits_a_character() {
    let mut message = message();
    message.receive("é");
    message.reveal(at(0), None);
    message.receive(&format!("é{}", "日本語".repeat(20)));
    message.reveal(at(200), Some(Duration::from_millis(1)));
    assert!(message.received().is_char_boundary(message.pushed));
}

proptest! {
    /// Paced or not, the committed output and the end are what `render` makes of the
    /// whole text.
    #[test]
    fn pacing_keeps_the_rendered_text(
        pieces in prop::collection::vec("[a-z #*`\\n-]{0,12}", 1..12),
        gaps in prop::collection::vec(0_i64..300, 12),
    ) {
        let mut message = message();
        let mut text = String::new();
        let mut out = String::new();
        let mut time = 0;
        for (piece, gap) in pieces.iter().zip(gaps) {
            time += gap;
            text.push_str(piece);
            message.receive(&text);
            out.push_str(&message.reveal(at(time), Some(Duration::from_millis(16))));
        }
        out.push_str(&message.finish());
        assert_eq!(out, render(&text, &RenderOptions::new(40)));
    }
}
