use pretty_assertions::assert_eq;

use super::cut_lines;

#[test]
fn a_long_diff_is_cut_with_a_count_of_the_rest() {
    let text = "a\nb\nc\nd\n";
    assert_eq!(cut_lines(text, 4), text);
    assert_eq!(cut_lines(text, 2), "a\nb\n... 2 more lines\n");
}
