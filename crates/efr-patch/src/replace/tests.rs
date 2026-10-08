use pretty_assertions::assert_eq;

use super::{Occurrences, Replacement, replace};
use crate::{NearLine, PatchError};

#[test]
fn one_unique_occurrence_is_replaced() {
    assert_eq!(
        replace("let a = 1;\nlet b = 2;\n", "a = 1", "a = 3", Occurrences::One),
        Ok(Replacement { text: "let a = 3;\nlet b = 2;\n".to_owned(), count: 1 })
    );
}

#[test]
fn several_occurrences_are_not_unique() {
    assert_eq!(
        replace("x x x", "x", "y", Occurrences::One),
        Err(PatchError::NotUnique { count: 3 })
    );
}

#[test]
fn all_replaces_every_occurrence_and_counts_them() {
    assert_eq!(
        replace("x x x", "x", "yy", Occurrences::All),
        Ok(Replacement { text: "yy yy yy".to_owned(), count: 3 })
    );
    assert_eq!(
        replace("x", "x", "", Occurrences::All),
        Ok(Replacement { text: String::new(), count: 1 })
    );
}

#[test]
fn the_match_is_exact() {
    assert!(matches!(
        replace("say \u{201C}hi\u{201D}\n", "say \"hi\"", "x", Occurrences::One),
        Err(PatchError::NotFound { .. })
    ));
    assert!(matches!(
        replace("a  \n", "a\n", "b\n", Occurrences::One),
        Err(PatchError::NotFound { .. })
    ));
}

#[test]
fn a_text_across_lines_is_replaced() {
    assert_eq!(
        replace("a\r\nb\r\nc\r\n", "a\r\nb\r\n", "B\r\n", Occurrences::One),
        Ok(Replacement { text: "B\r\nc\r\n".to_owned(), count: 1 })
    );
}

#[test]
fn an_old_text_that_does_not_occur_shows_the_nearest_lines() {
    let text = "fn one() {}\nfn two() {\n    work();\n}\nfn three() {}\n";
    assert_eq!(
        replace(text, "\nfn two() {\n    rest();\n}\n", "", Occurrences::One),
        Err(PatchError::NotFound {
            nearest: vec![
                NearLine { number: 1, text: "fn one() {}".to_owned() },
                NearLine { number: 2, text: "fn two() {".to_owned() },
                NearLine { number: 3, text: "    work();".to_owned() },
                NearLine { number: 4, text: "}".to_owned() },
                NearLine { number: 5, text: "fn three() {}".to_owned() },
            ],
        })
    );
}

#[test]
fn an_empty_old_text_is_refused() {
    assert_eq!(replace("abc", "", "x", Occurrences::All), Err(PatchError::EmptyOld));
}
