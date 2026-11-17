use pretty_assertions::assert_eq;

use super::{
    FORMAT_CHARS, INPUT_WAITS, NOTICE_BODY_MAX, QUESTION_WAITS, is_format, notice_body, one_line,
};

#[test]
fn a_notice_body_names_the_place_and_what_waits() {
    assert_eq!(notice_body("app", QUESTION_WAITS), "app: a question waits");
    assert_eq!(notice_body("~", INPUT_WAITS), "~: a command waits for input");
    assert_eq!(notice_body("", "done in 4m 12s"), "done in 4m 12s");
}

#[test]
fn a_notice_body_carries_no_control_or_format_character() {
    let body = notice_body("a\x1b]777;notify;x\x07pp\n", "q\u{202e}\u{9b}\tx");
    assert_eq!(body, "a ]777;notify;x pp : q\u{fffd}  x");
    assert!(!body.chars().any(|c| c.is_control() || is_format(c)), "{body:?}");
}

#[test]
fn a_long_notice_body_is_cut_to_its_maximum_with_dots() {
    let place = "é".repeat(300);
    let body = notice_body(&place, QUESTION_WAITS);
    assert_eq!(body.chars().count(), NOTICE_BODY_MAX);
    assert!(body.ends_with("éé..."), "{body}");

    // NOTE: `<197 x>: y` has exactly 200 characters, so it stays whole.
    let fits = "x".repeat(NOTICE_BODY_MAX - 3);
    assert_eq!(notice_body(&fits, "y"), format!("{fits}: y"));
    let over = "x".repeat(NOTICE_BODY_MAX - 2);
    assert_eq!(notice_body(&over, "y"), format!("{fits}..."));
    assert_eq!(one_line("", NOTICE_BODY_MAX), "");
}

#[test]
fn bidi_controls_zero_width_characters_and_tags_are_format_characters() {
    for c in [
        '\u{ad}',
        '\u{61c}',
        '\u{200b}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{202a}',
        '\u{202e}',
        '\u{2060}',
        '\u{2066}',
        '\u{2069}',
        '\u{feff}',
        '\u{e0001}',
        '\u{e0041}',
        '\u{e007f}',
    ] {
        assert!(is_format(c), "U+{:04X}", u32::from(c));
    }
}

#[test]
fn letters_marks_spaces_and_emoji_are_not() {
    for c in ['a', 'é', '日', '\u{301}', ' ', '\u{a0}', '\u{2028}', '\u{1f600}', '\u{fffd}', '\n']
    {
        assert!(!is_format(c), "U+{:04X}", u32::from(c));
    }
}

#[test]
fn the_ranges_are_ordered_and_apart() {
    for pair in FORMAT_CHARS.windows(2) {
        let [(first, last), (next, _)] = pair else { continue };
        assert!(first <= last && u32::from(*last) + 1 < u32::from(*next), "{pair:?}");
    }
}
