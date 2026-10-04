use pretty_assertions::assert_eq;

use super::*;
use crate::test_sink::{Call, TestSink};

/// What the recorder holds after vt100 parsed `bytes`.
fn parse(bytes: &[u8]) -> Recorder {
    let mut parser = vt100::Parser::new_with_callbacks(2, 10, 0, Recorder::default());
    parser.process(bytes);
    std::mem::take(parser.callbacks_mut())
}

#[test]
fn bel_is_a_bell() {
    assert_eq!(parse(b"a\x07b\x07").notices, [Notice::Bell, Notice::Bell]);
}

#[test]
fn osc_0_and_osc_2_set_the_title_with_either_terminator() {
    let recorder = parse(b"\x1b]0;first\x07\x1b]2;second\x1b\\");
    assert_eq!(recorder.title(), Some("second"));
    assert_eq!(
        recorder.notices,
        [Notice::Title("first".to_owned()), Notice::Title("second".to_owned())]
    );
}

#[test]
fn osc_1_names_the_icon_and_leaves_the_title_alone() {
    let recorder = parse(b"\x1b]1;icon\x07");
    assert_eq!(recorder.title(), None);
    assert_eq!(recorder.notices, []);
}

#[test]
fn a_title_with_semicolons_is_kept_whole() {
    for osc in ["0", "2"] {
        let recorder = parse(format!("\x1b]{osc};make; test;\x07").as_bytes());
        assert_eq!(recorder.title(), Some("make; test;"), "OSC {osc}");
        assert_eq!(recorder.notices, [Notice::Title("make; test;".to_owned())], "OSC {osc}");
    }
}

#[test]
fn a_title_keeps_the_fifteen_pieces_vte_keeps() {
    // vte keeps 16 OSC parameters, the first being the OSC number.
    let pieces: Vec<String> = (1..=20).map(|piece| piece.to_string()).collect();
    let recorder = parse(format!("\x1b]2;{}\x07", pieces.join(";")).as_bytes());
    assert_eq!(recorder.title(), Some(pieces[..15].join(";").as_str()));
}

#[test]
fn an_empty_title_clears_the_title_and_is_still_a_change() {
    let recorder = parse(b"\x1b]2;vim\x07\x1b]2;\x07");
    assert_eq!(recorder.title(), None);
    assert_eq!(recorder.notices, [Notice::Title("vim".to_owned()), Notice::Title(String::new())]);
}

#[test]
fn a_long_title_is_cut_on_a_character_boundary() {
    let mut osc = b"\x1b]2;".to_vec();
    osc.extend(std::iter::repeat_n(b'a', MAX_TITLE_BYTES - 1));
    osc.extend("\u{e9}tail".as_bytes());
    osc.push(0x07);
    let recorder = parse(&osc);
    let title = recorder.title().unwrap();
    assert_eq!(title.len(), MAX_TITLE_BYTES - 1);
    assert!(title.bytes().all(|byte| byte == b'a'));
}

#[test]
fn a_title_that_is_not_utf8_is_repaired() {
    let recorder = parse(b"\x1b]2;menu\xff\x07");
    assert_eq!(recorder.title(), Some("menu\u{fffd}"));
}

#[test]
fn osc_7_keeps_the_raw_url_in_both_forms() {
    let recorder = parse(b"\x1b]7;file://arch/tmp/a%20b\x07");
    assert_eq!(recorder.pwd(), Some("file://arch/tmp/a%20b"));
    let recorder = parse(b"\x1b]7;kitty-shell-cwd://arch/tmp/a b\x1b\\");
    assert_eq!(recorder.pwd(), Some("kitty-shell-cwd://arch/tmp/a b"));
}

#[test]
fn osc_7_is_no_title_change() {
    assert_eq!(parse(b"\x1b]7;file://arch/tmp\x07").notices, []);
}

#[test]
fn a_url_with_semicolons_is_kept_whole() {
    let recorder = parse(b"\x1b]7;file://arch/tmp/a;b;c\x07");
    assert_eq!(recorder.pwd(), Some("file://arch/tmp/a;b;c"));
}

#[test]
fn an_empty_url_clears_the_pwd() {
    let recorder = parse(b"\x1b]7;file://arch/tmp\x07\x1b]7;\x07");
    assert_eq!(recorder.pwd(), None);
}

#[test]
fn osc_7_without_a_url_changes_nothing() {
    let recorder = parse(b"\x1b]7;file://arch/tmp\x07\x1b]7\x07");
    assert_eq!(recorder.pwd(), Some("file://arch/tmp"));
}

#[test]
fn a_long_url_is_cut() {
    let mut osc = b"\x1b]7;file://arch/".to_vec();
    osc.extend(std::iter::repeat_n(b'd', MAX_PWD_BYTES));
    osc.push(0x07);
    assert_eq!(parse(&osc).pwd().map(str::len), Some(MAX_PWD_BYTES));
}

#[test]
fn other_oscs_are_ignored() {
    let recorder =
        parse(b"\x1b]133;A;cl=line\x07\x1b]52;c;aGk=\x07\x1b]10;?\x07\x1b]8;;http://x\x07");
    assert_eq!(recorder.title(), None);
    assert_eq!(recorder.pwd(), None);
    assert_eq!(recorder.notices, []);
}

#[test]
fn drain_hands_the_notices_over_in_order_once() {
    let mut recorder = parse(b"\x07\x1b]2;build\x07\x07");
    let mut sink = TestSink::default();
    recorder.drain_into(&mut sink);
    assert_eq!(sink.take(), [Call::Bell, Call::Title("build".to_owned()), Call::Bell]);
    recorder.drain_into(&mut sink);
    assert_eq!(sink.take(), []);
    assert_eq!(recorder.title(), Some("build"));
}
