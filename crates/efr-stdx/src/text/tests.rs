use super::{FORMAT_CHARS, is_format};

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
