use bytes::Bytes;
use efr_screen::{Cursor, Screen, ScreenActor, ScreenEvent, Seq, Size};
use pretty_assertions::assert_eq;

use super::{GhosttyConfig, GhosttyScreen, factory, factory_with};
use crate::testing::{Call, Recorder, texts};

fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

fn screen(cols: u16, rows: u16) -> GhosttyScreen {
    GhosttyScreen::new(size(cols, rows), &GhosttyConfig::default()).unwrap()
}

fn feed(screen: &mut GhosttyScreen, bytes: &[u8]) -> Recorder {
    let mut sink = Recorder::default();
    screen.feed(bytes, &mut sink);
    sink
}

#[test]
fn a_new_screen_is_blank_at_its_size() {
    let mut screen = screen(10, 3);
    let snapshot = screen.snapshot(100);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(texts(&snapshot.rows), vec!["", "", ""]);
    assert_eq!(snapshot.scrollback, Vec::new());
    assert_eq!(screen.cursor(), Cursor { row: 0, col: 0, hidden: false });
    assert_eq!(screen.title(), None);
    assert_eq!(screen.pwd(), None);
}

#[test]
fn a_zero_size_becomes_one_cell() {
    let mut screen = screen(0, 0);
    assert_eq!(screen.snapshot(0).size, size(1, 1));
    screen.resize(0, 5, &mut Recorder::default());
    assert_eq!(screen.snapshot(0).size, size(1, 5));
}

#[test]
fn feed_renders_text_and_moves_the_cursor() {
    let mut screen = screen(10, 3);
    let sink = feed(&mut screen, b"hello\r\nworld");
    assert_eq!(sink.calls, Vec::new());
    assert_eq!(texts(&screen.snapshot(0).rows), vec!["hello", "world", ""]);
    assert_eq!(screen.cursor(), Cursor { row: 1, col: 5, hidden: false });
    assert_eq!(texts(&[screen.row(1)]), vec!["world"]);
}

#[test]
fn terminal_queries_are_answered_through_the_sink() {
    let mut screen = screen(10, 2);
    assert_eq!(feed(&mut screen, b"\x1b[c").replies(), b"\x1b[?62;22c".to_vec());
    assert_eq!(feed(&mut screen, b"ab\x1b[6n").replies(), b"\x1b[1;3R".to_vec());
    assert_eq!(feed(&mut screen, b"\x1b[?7$p").replies(), b"\x1b[?7;1$y".to_vec());
}

#[test]
fn osc_10_and_11_are_answered_with_the_configured_colours() {
    let mut screen = screen(10, 2);
    let sink = feed(&mut screen, b"\x1b]10;?\x07\x1b]11;?\x1b\\");
    assert_eq!(
        sink.replies(),
        b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:2828/2c2c/3434\x1b\\".to_vec()
    );
}

#[test]
fn without_default_colours_osc_10_and_11_go_unanswered() {
    let config = GhosttyConfig { foreground: None, background: None, ..GhosttyConfig::default() };
    let mut screen = GhosttyScreen::new(size(10, 2), &config).unwrap();
    assert_eq!(feed(&mut screen, b"\x1b]10;?\x07\x1b]11;?\x07").calls, Vec::new());
}

#[test]
fn bells_and_titles_reach_the_sink_in_order() {
    let mut screen = screen(10, 2);
    let sink = feed(&mut screen, b"\x07\x1b]2;make\x07\x07");
    assert_eq!(sink.calls, vec![Call::Bell, Call::Title("make".to_owned()), Call::Bell]);
    assert_eq!(screen.title(), Some("make"));
}

#[test]
fn pwd_is_the_raw_osc_7_url() {
    let mut screen = screen(10, 2);
    feed(&mut screen, b"\x1b]7;file://arch/home/egg/My%20Docs\x07");
    assert_eq!(screen.pwd(), Some("file://arch/home/egg/My%20Docs"));
    feed(&mut screen, b"\x1b]7;kitty-shell-cwd://arch/tmp/x y\x07");
    assert_eq!(screen.pwd(), Some("kitty-shell-cwd://arch/tmp/x y"));
}

#[test]
fn resize_changes_the_grid_and_keeps_the_text() {
    let mut screen = screen(5, 2);
    feed(&mut screen, b"hey");
    let mut sink = Recorder::default();
    screen.resize(10, 3, &mut sink);
    assert_eq!(sink.calls, Vec::new());
    let snapshot = screen.snapshot(0);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(texts(&snapshot.rows), vec!["hey", "", ""]);
}

#[test]
fn resize_reports_in_band_when_the_program_asked_for_it() {
    let mut screen = screen(10, 3);
    feed(&mut screen, b"\x1b[?2048h");
    let mut sink = Recorder::default();
    screen.resize(31, 6, &mut sink);
    assert_eq!(sink.replies(), b"\x1b[48;6;31;96;248t".to_vec());
}

#[test]
fn the_scrollback_limit_drops_old_rows() {
    let config = GhosttyConfig { scrollback_lines: 10, ..GhosttyConfig::default() };
    let mut screen = GhosttyScreen::new(size(20, 2), &config).unwrap();
    let lines: Vec<u8> = (0..3000).flat_map(|line| format!("{line}\r\n").into_bytes()).collect();
    feed(&mut screen, &lines);
    let kept = screen.snapshot(usize::MAX).scrollback.len();
    // libghostty-vt prunes whole pages, so it keeps more than ten lines, but far
    // fewer than three thousand.
    assert!((10..3000).contains(&kept), "{kept} rows of scrollback kept");
}

#[test]
fn the_factory_builds_the_screen_on_the_actor_thread() {
    let (handle, mut events) =
        ScreenActor::spawn("screen-ghostty-test", factory(size(8, 2)), size(12, 3)).unwrap();
    handle.feed_blocking(Bytes::from_static(b"hi\x1b[c"), Seq::ZERO).unwrap();
    assert_eq!(
        events.blocking_recv(),
        Some(ScreenEvent::PtyReply(Bytes::from_static(b"\x1b[?62;22c")))
    );
    let capture = handle.snapshot_blocking(0).unwrap();
    assert_eq!(capture.at, Seq::new(5));
    assert_eq!(capture.snapshot.size, size(12, 3));
    assert_eq!(texts(&capture.snapshot.rows), vec!["hi", "", ""]);
    handle.shutdown_blocking().unwrap();
    assert_eq!(events.blocking_recv(), None);
}

#[test]
fn factory_with_applies_the_config() {
    let config = GhosttyConfig { background: Some([0, 0, 0]), ..GhosttyConfig::default() };
    let (handle, mut events) =
        ScreenActor::spawn("screen-ghostty-test", factory_with(size(8, 2), config), size(8, 2))
            .unwrap();
    handle.feed_blocking(Bytes::from_static(b"\x1b]11;?\x07"), Seq::ZERO).unwrap();
    assert_eq!(
        events.blocking_recv(),
        Some(ScreenEvent::PtyReply(Bytes::from_static(b"\x1b]11;rgb:0000/0000/0000\x07")))
    );
}

#[test]
fn the_default_config_uses_ghosttys_colours() {
    let config = GhosttyConfig::default();
    assert_eq!(config.scrollback_lines, 5000);
    assert_eq!(config.foreground, Some([0xff, 0xff, 0xff]));
    assert_eq!(config.background, Some([0x28, 0x2c, 0x34]));
}
