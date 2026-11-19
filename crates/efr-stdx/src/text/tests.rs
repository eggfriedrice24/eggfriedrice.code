use std::time::Duration;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{
    FORMAT_CHARS, INPUT_WAITS, NOTICE_BODY_MAX, QUESTION_WAITS, byte_size, hex, is_format,
    notice_body, notice_ended, notice_place, omitted_marker, one_line, span,
};

#[rstest]
#[case(0, "0s")]
#[case(45, "45s")]
#[case(5 * 60 + 3, "5m 3s")]
#[case(2 * 3_600 + 5 * 60 + 9, "2h 5m")]
#[case(3 * 86_400 + 4 * 3_600 + 59, "3d 4h")]
fn a_span_has_its_two_largest_units(#[case] seconds: u64, #[case] want: &str) {
    assert_eq!(span(seconds), want);
}

#[test]
fn the_end_of_a_turn_names_the_place_and_the_time() {
    let took = Duration::from_secs(4 * 60 + 12);
    assert_eq!(notice_ended("app", took, false), "app: done in 4m 12s");
    assert_eq!(notice_ended("app", Duration::from_secs(63), true), "app: failed after 1m 3s");
    assert_eq!(notice_ended("", Duration::from_millis(900), false), "done in 0s");
}

#[rstest]
#[case(0, "0 B")]
#[case(999, "999 B")]
#[case(2_000, "2.0 KB")]
#[case(9_999, "9.9 KB")]
#[case(14_200, "14 KB")]
#[case(999_999, "999 KB")]
#[case(2_180_000, "2.1 MB")]
#[case(310_000_000, "310 MB")]
#[case(1_250_000_000, "1.2 GB")]
#[case(u64::MAX, "18446744073 GB")]
fn a_byte_size_has_one_decimal_only_under_ten(#[case] bytes: u64, #[case] want: &str) {
    assert_eq!(byte_size(bytes), want);
}

#[test]
fn hex_has_two_lowercase_digits_for_each_byte() {
    assert_eq!(hex(&[]), "");
    assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
}

#[test]
fn the_omitted_marker_stands_on_a_line_of_its_own() {
    assert_eq!(omitted_marker(1_048_576), "\n[... 1048576 bytes omitted ...]\n");
}

#[test]
fn the_place_is_the_project_else_the_last_part_of_the_dir_else_home() {
    let cwd = std::path::Path::new("/home/me/p/app/src");
    assert_eq!(notice_place(Some("app".to_owned()), Some(cwd)), "app");
    assert_eq!(notice_place(None, Some(cwd)), "src");
    assert_eq!(notice_place(None, Some(std::path::Path::new("/"))), "~");
    assert_eq!(notice_place(None, None), "~");
}

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
