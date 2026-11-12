//! The frames that write only what changed leave the screen that the frames that write
//! the whole live zone leave: a turn with a paragraph taller than the screen, a list, a
//! running call with its output, a second message, the status row at its ticks and the
//! input row, played through both kinds of view on a strict screen
//! (`testing::screen`), at several sizes and both ways of counting widths.

use efr_protocol::Event;
use efr_render::{RenderOptions, WidthMethod};

use super::{
    ALL, at, call_completed, completed, grown, output, tool_started, turn_started, updated,
};
use crate::follow::view::TurnView;
use crate::terminal::Size;
use crate::testing::screen::Screen;
use crate::testing::{readable, turn};

/// What happens next in the turn.
enum Act {
    Event(Box<Event>),
    Frame,
    Tick,
}

/// A turn that streams `first`, runs a call, then streams `second`, in pieces of 11
/// bytes with a frame after each and a tick after every third.
fn script(first: &str, second: &str) -> Vec<(i64, Act)> {
    let mut acts = vec![(0, Act::Event(Box::new(turn_started()))), (0, Act::Frame)];
    let mut now = 0;
    for (index, text) in [(0, first), (1, second)] {
        let mut sent = 0;
        while sent < text.len() {
            let mut end = (sent + 11).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            now += 17;
            let event = if sent == 0 {
                updated(index, &text[..end])
            } else {
                grown(index, &text[..sent], &text[..end])
            };
            acts.push((now, Act::Event(Box::new(event))));
            acts.push((now, Act::Frame));
            if (end / 11) % 3 == 0 {
                acts.push((now + 50, Act::Tick));
            }
            sent = end;
        }
        acts.push((now, Act::Event(Box::new(completed(index, text)))));
        acts.push((now, Act::Frame));
        if index == 0 {
            acts.push((now, Act::Event(Box::new(tool_started("cargo test --workspace")))));
            acts.push((now, Act::Frame));
            for lines in ["one\n", "one\ntwo\n", "one\ntwo\nthree\nfour\n"] {
                now += 120;
                acts.push((now, Act::Event(Box::new(output(lines)))));
                acts.push((now, Act::Frame));
                acts.push((now + 100, Act::Tick));
            }
            acts.push((now, Act::Event(Box::new(call_completed(0)))));
            acts.push((now, Act::Frame));
        }
    }
    acts
}

fn view(cols: u16, method: WidthMethod, changes: bool) -> TurnView {
    let options = RenderOptions::new(cols).with_width_method(method);
    let mut view = TurnView::new(turn(), options).with_look(ALL).with_input();
    if changes {
        view = view.writing_changes();
    }
    view.start();
    view.show_row(true);
    view
}

/// Plays `acts` through both kinds of view on screens of `size`, and checks after each
/// write that both screens show the same.
fn play(acts: &[(i64, Act)], size: Size, method: WidthMethod) {
    let mut changes = view(size.cols, method, true);
    let mut whole = view(size.cols, method, false);
    let mut left = Screen::new(size.cols, size.rows, method);
    let mut right = Screen::new(size.cols, size.rows, method);
    let mut written = Vec::new();
    for (millis, act) in acts {
        let (one, two) = match act {
            Act::Event(event) => {
                let one = changes.event(event, size, true).out;
                let two = whole.event(event, size, true).out;
                (one, two)
            }
            Act::Frame => (changes.frame(size, at(*millis)), whole.frame(size, at(*millis))),
            Act::Tick => (changes.tick(size, at(*millis)), whole.tick(size, at(*millis))),
        };
        left.write(&one);
        right.write(&two);
        written.push(readable(&one));
        assert!(
            left.shot() == right.shot(),
            "{size:?} {method:?}, the writes:\n{}\nthe screen of the frames that write only what changed:\n{}\nthe screen of the frames that write all:\n{}",
            written.join("\n"),
            left.text(),
            right.text(),
        );
    }
    let mut ended = changes.close().out;
    ended.push_str(&changes.frame(size, at(60_000)));
    let mut ended_too = whole.close().out;
    ended_too.push_str(&whole.frame(size, at(60_000)));
    left.write(&ended);
    right.write(&ended_too);
    assert!(left.shot() == right.shot(), "after the end:\n{}\n{}", left.text(), right.text());
}

#[test]
fn a_turn_shows_the_same_screen_with_frames_that_write_only_what_changed() {
    let mut first =
        String::from("A first answer with **bold** words, `code` and \u{65e5}\u{672c}. ");
    for n in 0..60 {
        first.push_str(&format!("word{n} "));
    }
    first.push_str("\n\n- one item\n- a second item that is long enough to wrap at forty\n\n");
    let second = "Done: the \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} tests pass.\n";
    let acts = script(&first, second);
    for (cols, rows) in [(40, 12), (80, 24), (23, 8)] {
        for method in [WidthMethod::CodePoint, WidthMethod::Grapheme] {
            play(&acts, Size { cols, rows }, method);
        }
    }
}
