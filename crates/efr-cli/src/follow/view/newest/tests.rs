use efr_render::WidthMethod;
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::{Growing, ToolInput, input_words};
use crate::testing::now;

const METHOD: WidthMethod = WidthMethod::Grapheme;

/// The mark of more text.
const MORE: &str = efr_render::Glyphs::UNICODE.more;

#[test]
fn the_words_of_a_tool_input_are_one_line_without_the_escapes_of_json() {
    assert_eq!(
        input_words(r#"{"command":"python3 -c \"import sys\nprint(len(s.split()))\"\t"}"#),
        r#"{"command":"python3 -c "import sys print(len(s.split()))" "}"#
    );
    // A real newline of a freeform input, such as a patch, is a space too, and a
    // control character that the daemon let through shows as its picture.
    assert_eq!(
        input_words("*** Begin Patch\n+fn main() {}\x07"),
        "*** Begin Patch +fn main() {}\u{2407}"
    );
    // An escape that makes no space stays, and a backslash at the end of a part too.
    assert_eq!(input_words(r"C:\\tmp \u00e9 \"), r"C:\tmp \u00e9 \");
}

#[test]
fn a_growing_text_keeps_only_the_new_part_of_a_repeated_one_and_parts_a_gap() {
    let mut text = Growing::from(10);
    assert_eq!(text.take(10, "one two"), "one two");
    // A part that repeats bytes that the text holds adds only the rest.
    assert_eq!(text.take(14, "two three"), " three");
    assert_eq!(text.tail, "one two three");
    // A part after a gap keeps its words apart from the ones before it.
    assert_eq!(text.take(40, "four"), "four");
    assert_eq!(text.tail, "one two three four");
    assert_eq!(text.end, 44);
}

#[test]
fn a_tool_input_shows_its_newest_words_once_it_has_grown_for_a_second() {
    let mut input = ToolInput::new(0);
    input.take(30, r#"{"command":"cat notes.txt | wc"#);
    input.at(now());
    assert_eq!(input.words(MORE, METHOD), None, "no words before the first second");
    // The next tail overlaps the one before it: only its new bytes count.
    input.take(35, r#"txt | wc -l"}"#);
    input.at(now() + SignedDuration::from_millis(999));
    assert_eq!(input.words(MORE, METHOD), None);
    input.at(now() + SignedDuration::from_millis(1_000));
    assert_eq!(
        input.words(MORE, METHOD).as_deref(),
        Some(r#"{"command":"cat notes.txt | wc -l"}"#)
    );
}

#[test]
fn the_newest_words_of_a_long_input_fit_in_50_columns_and_move_in_steps() {
    let mut input = ToolInput::new(0);
    let mut written = String::new();
    let mut shown = Vec::new();
    for word in 0..40 {
        let more = format!(" w{word:02}");
        written.push_str(&more);
        let tail = &written[written.len().saturating_sub(16)..];
        input.take(written.len() as u64, tail);
        input.at(now() + SignedDuration::from_millis(100 * word));
        if let Some(words) = input.words(MORE, METHOD) {
            assert!(efr_render::text_width(&words, METHOD) <= 50, "{words:?}");
            shown.push(words);
        }
    }
    assert_eq!(
        shown.first().map(String::as_str),
        Some("w00 w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")
    );
    assert_eq!(
        shown.last().map(String::as_str),
        Some("\u{2026} w28 w29 w30 w31 w32 w33 w34 w35 w36 w37 w38 w39")
    );
    // The start moves four words at a time, never one.
    let starts: Vec<&str> = shown
        .iter()
        .filter_map(|words| words.strip_prefix("\u{2026} ")?.split(' ').next())
        .collect();
    let mut moves: Vec<&str> = starts.clone();
    moves.dedup();
    assert_eq!(moves, ["w04", "w08", "w12", "w16", "w20", "w24", "w28"]);
}
