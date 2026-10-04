use pretty_assertions::assert_eq;

use crate::testing::{Call, Recorder, terminal};

#[test]
fn replies_reach_the_sink_in_one_call_at_the_drain() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"ab\x1b[c\x1b[6n");
    let mut sink = Recorder::default();
    effects.drain(&mut sink);
    assert_eq!(sink.calls, vec![Call::Reply(b"\x1b[?62;22c\x1b[1;3R".to_vec())]);
}

#[test]
fn bells_and_titles_keep_the_order_they_happened_in() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x07\x1b]2;build\x07\x07\x1b]0;done\x1b\\");
    let mut sink = Recorder::default();
    effects.drain(&mut sink);
    assert_eq!(
        sink.calls,
        vec![
            Call::Bell,
            Call::Title("build".to_owned()),
            Call::Bell,
            Call::Title("done".to_owned()),
        ]
    );
}

#[test]
fn notices_come_before_the_replies_of_the_same_feed() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b[5n\x07");
    let mut sink = Recorder::default();
    effects.drain(&mut sink);
    assert_eq!(sink.calls, vec![Call::Bell, Call::Reply(b"\x1b[0n".to_vec())]);
}

#[test]
fn a_drain_empties_the_buffer() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"\x1b[c\x07");
    effects.drain(&mut Recorder::default());
    let mut sink = Recorder::default();
    effects.drain(&mut sink);
    assert_eq!(sink.calls, Vec::new());
}

#[test]
fn plain_output_produces_no_calls() {
    let (mut terminal, effects) = terminal(10, 2);
    terminal.vt_write(b"hello\r\n\x1b[1mworld");
    let mut sink = Recorder::default();
    effects.drain(&mut sink);
    assert_eq!(sink.calls, Vec::new());
}
