use std::sync::{Arc, Mutex};

use efr_render::{RenderOptions, render};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};

use super::Message;

fn message() -> Message {
    Message::new(0, RenderOptions::new(40))
}

#[test]
fn all_text_that_arrived_goes_into_the_renderer_at_once() {
    let mut message = message();
    message.receive("a");
    message.push_all();
    // A persisted update 200 ms later: no part of it waits for a later frame.
    message.receive(&format!("a{}", "b".repeat(120)));
    assert!(message.waiting());
    message.push_all();
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
fn a_delta_at_an_offset_adds_only_what_is_new() {
    let mut message = message();
    message.receive_at(0, "drafts ran");
    message.receive_at(7, "ran ahead");
    assert_eq!(message.received(), "drafts ran ahead");
    // An update behind the drafts.
    assert_eq!(logs(|| message.receive_at(0, "drafts")), "");
    assert_eq!(message.received(), "drafts ran ahead");
    let rewrite = logs(|| message.receive_at(7, "walked"));
    assert!(rewrite.contains("rewrote sent text"), "{rewrite}");
    assert_eq!(message.received(), "drafts ran ahead");
}

proptest! {
    /// A delta at an offset changes the text as the text up to the offset and the
    /// delta would, and it logs the same.
    #[test]
    fn a_delta_at_an_offset_is_the_text_before_it_and_the_delta(
        start in "[ab\u{e4}]{0,8}",
        deltas in prop::collection::vec(("[ab\u{e4}]{0,6}", 0_usize..12), 1..8),
    ) {
        let (mut by_delta, mut by_text) = (message(), message());
        by_delta.receive(&start);
        by_text.receive(&start);
        for (delta, back) in &deltas {
            let held = by_text.received();
            let mut offset = held.len().saturating_sub(*back);
            while !held.is_char_boundary(offset) {
                offset -= 1;
            }
            let text = format!("{}{delta}", &held[..offset]);
            let logged_text = logs(|| by_text.receive(&text));
            let logged_delta = logs(|| by_delta.receive_at(offset, delta));
            assert_eq!(by_delta.received(), by_text.received());
            assert_eq!(logged_delta.is_empty(), logged_text.is_empty());
        }
    }

    /// Pushed in any pieces, the committed output and the end are what `render` makes
    /// of the whole text.
    #[test]
    fn pushing_keeps_the_rendered_text(
        pieces in prop::collection::vec("[a-z #*`\\n-]{0,12}", 1..12),
    ) {
        let mut message = message();
        let mut text = String::new();
        let mut out = String::new();
        for piece in &pieces {
            text.push_str(piece);
            message.receive(&text);
            out.push_str(&message.push_all());
        }
        out.push_str(&message.finish());
        assert_eq!(out, render(&text, &RenderOptions::new(40)));
    }
}
