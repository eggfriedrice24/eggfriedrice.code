use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

fn score(title: &str, words: &[&str]) -> Option<i32> {
    Scorer::new(words).score(title).map(|found| found.score)
}

fn ranges_of(title: &str, words: &[&str]) -> Vec<Range<u32>> {
    Scorer::new(words).score(title).map(|found| found.ranges).unwrap_or_default()
}

// The expected scores of these three are those of fzf's own tests of its first
// algorithm (`src/algo/algo_test.go`), written with the same constants.

#[test]
fn fzf_camel_case_example() {
    let expected = SCORE_MATCH * 3 + BONUS_CAMEL_123 + SCORE_GAP_START + SCORE_GAP_EXTENSION * 3;
    assert_eq!(score("fooBarbaz1", &["obz"]), Some(expected));
    assert_eq!(ranges_of("fooBarbaz1", &["obz"]), vec![2..4, 8..9]);
}

#[test]
fn fzf_white_space_example() {
    let expected = SCORE_MATCH * 3
        + BONUS_BOUNDARY_WHITE * BONUS_FIRST_CHAR_MULTIPLIER
        + BONUS_BOUNDARY_WHITE * 2
        + 2 * SCORE_GAP_START
        + 4 * SCORE_GAP_EXTENSION;
    assert_eq!(score("foo bar baz", &["fbb"]), Some(expected));
}

#[test]
fn fzf_consecutive_example() {
    let expected = SCORE_MATCH * 4 + BONUS_CAMEL_123 + BONUS_CONSECUTIVE * 2;
    assert_eq!(score("/AutomatorDocument.icns", &["rdoc"]), Some(expected));
    assert_eq!(ranges_of("/AutomatorDocument.icns", &["rdoc"]), vec![9..13]);
}

#[test]
fn smart_case() {
    assert!(score("Docker compose", &["docker"]).is_some());
    assert!(score("docker compose", &["Docker"]).is_none(), "an upper case letter matches case");
    assert!(score("Docker compose", &["Docker"]).is_some());
}

#[test]
fn every_word_must_match() {
    assert!(score("rsync to the laptop", &["rsync", "lap"]).is_some());
    assert!(score("rsync to the laptop", &["rsync", "desk"]).is_none());
    assert!(Scorer::new(&[]).score("anything").is_none(), "no word matches no title");
}

#[test]
fn a_gap_costs_points() {
    let tight = score("docker", &["dock"]).unwrap();
    let loose = score("d o c k", &["dock"]).unwrap();
    assert!(tight > loose, "{tight} > {loose}");
}

#[test]
fn a_word_start_beats_the_middle_of_a_word() {
    let start = score("the lap of it", &["lap"]).unwrap();
    let middle = score("the clap of it", &["lap"]).unwrap();
    assert!(start > middle, "{start} > {middle}");
}

#[test]
fn ranges_merge_and_follow_code_points() {
    assert_eq!(ranges_of("rsync to the laptop", &["rsync", "lap"]), vec![0..5, 13..16]);
    assert_eq!(ranges_of("über café", &["café"]), vec![5..9]);
}

#[test]
fn the_ideal_is_the_best_score() {
    let scorer = Scorer::new(&["docker", "compose"]);
    let found = scorer.score("docker compose").unwrap();
    assert_eq!(scorer.relative(&found), 1.0);
    let weaker = scorer.score("my d-o-c-k-e-r and its compose").unwrap();
    assert!(scorer.relative(&weaker) < 1.0);
    assert!(scorer.relative(&weaker) > 0.0);
}

#[test]
fn no_word_matches_inside_the_redaction_marker() {
    let title = "deploy with [***] to staging";
    for words in [&["***"][..], &["[***]"], &["*"], &["["], &["]"], &["h["], &["h[*"]] {
        assert_eq!(score(title, words), None, "words {words:?}");
    }
    assert_eq!(ranges_of(title, &["staging"]), vec![21..28], "the positions stay the title's");
    assert_eq!(ranges_of(title, &["[*", "deploy"]), vec![0..6], "only the word with letters");
    assert_eq!(ranges_of("a * b", &["a*"]), vec![0..1, 2..3], "a star outside the marker matches");
}

#[test]
fn a_word_without_a_letter_or_a_digit_is_left_out() {
    assert_eq!(score("a - b", &["-"]), None);
    assert_eq!(score("fix it", &["...", "  "]), None, "no word left matches no title");
    let scorer = Scorer::new(&["fix", "--"]);
    assert_eq!(scorer.ideal(), Scorer::new(&["fix"]).ideal());
    assert!(scorer.score("fix it").is_some());
}

proptest! {
    #[test]
    fn no_range_touches_a_redaction_marker(
        before in "[a-z*\\[\\] ]{0,12}",
        after in "[a-z*\\[\\] ]{0,12}",
        words in proptest::collection::vec("[a-z*\\[\\]]{1,4}", 1..3),
    ) {
        let title = format!("{before}{REDACTED}{after}");
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let start = u32::try_from(before.chars().count()).unwrap();
        let marker = start..start + u32::try_from(REDACTED.chars().count()).unwrap();
        if let Some(found) = Scorer::new(&words).score(&title) {
            for range in &found.ranges {
                prop_assert!(range.end <= marker.start || range.start >= marker.end, "{range:?}");
            }
        }
    }

    #[test]
    fn scorer_never_panics(title in "\\PC{0,100}", words in proptest::collection::vec("\\PC{0,8}", 0..4)) {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let scorer = Scorer::new(&words);
        if let Some(found) = scorer.score(&title) {
            let chars = u32::try_from(title.chars().count()).unwrap();
            for range in &found.ranges {
                prop_assert!(range.start < range.end && range.end <= chars);
            }
            let relative = scorer.relative(&found);
            prop_assert!((0.0..=1.0).contains(&relative));
        }
    }
}
