#![allow(
    clippy::single_range_in_vec_init,
    reason = "ranges of code points are the values under test"
)]

use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

fn needle(word: &str, prefix: bool) -> Needle {
    let tokens: Vec<String> = word
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    Needle::new(&tokens, prefix)
}

#[test]
fn fold_follows_remove_diacritics() {
    assert_eq!("Café Über ŁÓDŹ ñ".chars().map(fold).collect::<String>(), "cafe uber łodz n");
    assert_eq!(fold('Æ'), 'æ', "a letter with no diacritic only gets its lower case");
}

#[test]
fn find_matches_tokens_and_phrases() {
    let text = "rsync -a src/app to the Laptop, then laptops";
    let found =
        find(text, &[needle("rsync", false), needle("lap", true), needle("src/app", false)]);
    assert_eq!(found, vec![(0, 0..5), (2, 9..16), (1, 24..30), (1, 37..44)]);
    assert!(find(text, &[needle("lap", false)]).is_empty(), "a whole word is no prefix");
    assert!(find(text, &[needle("app src", false)]).is_empty(), "a phrase keeps its order");
}

#[test]
fn snippet_takes_the_first_line_with_the_most_words() {
    let text = "nothing here\nonly docker\ndocker compose up\ndocker compose again";
    let (line, ranges) = snippet(text, &[needle("docker", false), needle("compose", false)]);
    assert_eq!(line, "docker compose up");
    assert_eq!(ranges, vec![0..6, 7..14]);
}

#[test]
fn snippet_without_a_match_shows_the_first_line() {
    let (line, ranges) = snippet("\n  \nfirst words\nmore", &[needle("absent", false)]);
    assert_eq!((line.as_str(), ranges), ("first words", vec![]));
}

#[test]
fn a_long_line_is_cut_around_its_first_match() {
    let line = format!("{}needle{}", "a ".repeat(300), " b".repeat(300));
    let (cut_line, ranges) = snippet(&line, &[needle("needle", false)]);
    assert_eq!(cut_line.chars().count(), LINE_CHARS);
    assert!(cut_line.starts_with(ELLIPSIS) && cut_line.ends_with(ELLIPSIS));
    let range = ranges[0].clone();
    let shown: String = cut_line
        .chars()
        .skip(range.start as usize)
        .take((range.end - range.start) as usize)
        .collect();
    assert_eq!(shown, "needle");
}

#[test]
fn an_early_match_keeps_the_start_of_the_line() {
    let line = format!("needle {}", "word ".repeat(200));
    let (cut_line, ranges) = cut(&line, &[0..6], LINE_CHARS);
    assert!(cut_line.starts_with("needle") && cut_line.ends_with(ELLIPSIS));
    assert_eq!(cut_line.chars().count(), LINE_CHARS);
    assert_eq!(ranges, vec![0..6]);
}

#[test]
fn a_match_near_the_end_ends_the_line() {
    let line = format!("{}needle", "word ".repeat(200));
    let (cut_line, ranges) = cut(&line, &[1000..1006], LINE_CHARS);
    assert!(cut_line.starts_with(ELLIPSIS) && cut_line.ends_with("needle"));
    assert!(cut_line.chars().count() <= LINE_CHARS);
    let end = u32::try_from(cut_line.chars().count()).unwrap();
    assert_eq!(ranges, vec![end - 6..end]);
}

#[test]
fn multi_byte_text() {
    let text = "日本語 の テキスト café";
    let (line, ranges) = snippet(text, &[needle("cafe", false)]);
    assert_eq!(line, text);
    assert_eq!(ranges, vec![11..15]);
}

#[test]
fn preview_keeps_matches_and_context() {
    let lines: Vec<(String, Vec<Range<u32>>)> = (0..20)
        .map(|i| (format!("line {i}"), if i == 5 || i == 15 { vec![0..4] } else { vec![] }))
        .collect();
    let shown: Vec<String> = preview(lines).into_iter().map(|(text, _)| text).collect();
    assert_eq!(shown, vec!["line 4", "line 5", "line 6", "line 14", "line 15", "line 16"]);
}

#[test]
fn preview_without_a_match_shows_the_first_lines() {
    let lines: Vec<(String, Vec<Range<u32>>)> =
        (0..20).map(|i| (format!("line {i}"), vec![])).collect();
    assert_eq!(preview(lines).len(), PREVIEW_LINES);
}

#[test]
fn preview_gives_matches_the_room_first() {
    let lines: Vec<(String, Vec<Range<u32>>)> = (0..40)
        .map(|i| (format!("line {i}"), if i % 3 == 0 { vec![0..4] } else { vec![] }))
        .collect();
    let shown = preview(lines);
    assert_eq!(shown.len(), PREVIEW_LINES);
    assert!(shown.iter().all(|(_, ranges)| !ranges.is_empty()));
}

proptest! {
    #[test]
    fn snippets_never_panic(text in "\\PC{0,500}", word in "[a-zé]{1,5}", prefix: bool) {
        let (line, ranges) = snippet(&text, &[needle(&word, prefix)]);
        let chars = u32::try_from(line.chars().count()).unwrap();
        prop_assert!(chars as usize <= LINE_CHARS);
        for range in ranges {
            prop_assert!(range.start < range.end && range.end <= chars);
        }
    }

    #[test]
    fn cut_never_panics(len in 0usize..600, start in 0u32..700, width in 0u32..50, max in 0usize..500) {
        let line = "x".repeat(len);
        let (text, ranges) = cut(&line, &[start..start + width], max);
        let chars = u32::try_from(text.chars().count()).unwrap();
        prop_assert!(chars as usize <= max);
        for range in ranges {
            prop_assert!(range.start < range.end && range.end <= chars);
        }
    }
}
