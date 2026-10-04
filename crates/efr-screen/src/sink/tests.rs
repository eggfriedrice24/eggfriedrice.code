use pretty_assertions::assert_eq;

use super::{Collector, Notice};
use crate::ScreenSink;

#[test]
fn replies_are_concatenated_in_order() {
    let mut collector = Collector::default();
    collector.pty_reply(b"\x1b[?62;22c");
    collector.pty_reply(b"\x1b[1;1R");
    assert_eq!(collector.take_replies(), b"\x1b[?62;22c\x1b[1;1R".to_vec());
}

#[test]
fn notices_keep_the_order_they_happened_in() {
    let mut collector = Collector::default();
    collector.bell();
    collector.title_changed("build");
    collector.bell();
    assert_eq!(
        collector.take_notices(),
        vec![Notice::Bell, Notice::Title("build".to_owned()), Notice::Bell]
    );
}

#[test]
fn taking_empties_the_buffers() {
    let mut collector = Collector::default();
    collector.pty_reply(b"x");
    collector.bell();
    let _ = collector.take_replies();
    let _ = collector.take_notices();
    assert_eq!(collector.take_replies(), Vec::<u8>::new());
    assert_eq!(collector.take_notices(), Vec::new());
}
