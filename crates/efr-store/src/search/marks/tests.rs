#![allow(
    clippy::single_range_in_vec_init,
    reason = "ranges of code points are the values under test"
)]

use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

fn marked(text: &str) -> String {
    text.replace('[', &OPEN.to_string()).replace(']', &CLOSE.to_string())
}

#[test]
fn markers_become_ranges() {
    assert_eq!(
        strip(&marked("rsync to the [laptop]")),
        ("rsync to the laptop".to_owned(), vec![13..19])
    );
    assert_eq!(strip(&marked("[a] b [c]")), ("a b c".to_owned(), vec![0..1, 4..5]));
    assert_eq!(strip("no marks"), ("no marks".to_owned(), vec![]));
}

#[test]
fn adjacent_ranges_merge() {
    assert_eq!(strip(&marked("[src][/a]")), ("src/a".to_owned(), vec![0..5]));
}

#[test]
fn a_marker_at_the_end() {
    assert_eq!(strip(&marked("to the [lap")), ("to the lap".to_owned(), vec![7..10]));
    assert_eq!(strip(&marked("to the lap[")), ("to the lap".to_owned(), vec![]));
    assert_eq!(strip(&marked("x]y")), ("xy".to_owned(), vec![]));
    assert_eq!(strip(&marked("[[x]]y")), ("xy".to_owned(), vec![0..1]));
}

#[test]
fn ranges_count_code_points() {
    assert_eq!(strip(&marked("café [über] 日本")), ("café über 日本".to_owned(), vec![5..9]));
    assert_eq!(strip(&marked("日本[語]")), ("日本語".to_owned(), vec![2..3]));
}

#[test]
fn lines_take_their_ranges() {
    let (text, ranges) = strip(&marked("one [two]\nthree\n[four] [fi\nve]"));
    assert_eq!(
        lines(&text, &ranges),
        vec![
            ("one two".to_owned(), vec![4..7]),
            ("three".to_owned(), vec![]),
            ("four fi".to_owned(), vec![0..4, 5..7]),
            ("ve".to_owned(), vec![0..2]),
        ]
    );
}

proptest! {
    #[test]
    fn strip_never_panics(text in "[a-zé日\\x02\\x03\\n ]{0,60}") {
        let (plain, ranges) = strip(&text);
        let chars = u32::try_from(plain.chars().count()).unwrap();
        let mut last_end = 0;
        for (index, range) in ranges.iter().enumerate() {
            prop_assert!(range.start < range.end && range.end <= chars);
            if index > 0 {
                prop_assert!(range.start > last_end, "sorted, apart, merged");
            }
            last_end = range.end;
        }
        let _ = lines(&plain, &ranges);
    }
}
