use pretty_assertions::assert_eq;

use super::*;

fn candidate(key: &'static str, text: Option<f64>, title: f64) -> Candidate<&'static str> {
    Candidate { key, text, title }
}

#[test]
fn the_formula() {
    assert_eq!(score(1.0, 0.0), 0.8);
    assert_eq!(score(0.0, 1.0), 0.8);
    assert_eq!(score(1.0, 1.0), 1.0);
    assert_eq!(score(0.5, 0.5), 0.5);
    assert!(score(1.0, 0.5) > score(1.0, 0.0), "a title match adds to a text match");
}

#[test]
fn buckets_of_twentieths() {
    assert_eq!(bucket(1.0), 20);
    assert_eq!(bucket(0.999), 19);
    assert_eq!(bucket(0.05), 1);
    assert_eq!(bucket(0.049), 0);
    assert_eq!(bucket(0.0), 0);
    assert_eq!(bucket(f64::NAN), 0);
}

#[test]
fn weights_favour_what_the_person_typed() {
    assert_eq!(weight(0), 2.0);
    assert_eq!(weight(1), 2.0);
    assert_eq!(weight(3), 1.5);
    assert_eq!(weight(2), 1.0);
    assert_eq!(hit_score(0, Some(-3.0)), 6.0);
    assert_eq!(hit_score(2, Some(-3.0)), 3.0);
    assert_eq!(hit_score(3, None), 1.5, "without bm25 the weight alone");
}

#[test]
fn text_scores_are_relative_to_the_best() {
    let ranked = buckets(vec![
        candidate("weak", Some(1.0), 0.0),
        candidate("best", Some(4.0), 0.0),
        candidate("both", Some(4.0), 1.0),
    ]);
    let order: Vec<(&str, u32)> = ranked.iter().map(|r| (r.key, r.bucket)).collect();
    assert_eq!(order, vec![("both", 20), ("best", 16), ("weak", 4)]);
}

#[test]
fn min_title_score_drops_a_weak_title_only_match() {
    let ranked = buckets(vec![
        candidate("weak title", None, MIN_TITLE_SCORE - 0.01),
        candidate("title", None, MIN_TITLE_SCORE),
        candidate("weak title and text", Some(1.0), 0.01),
    ]);
    let keys: Vec<&str> = ranked.iter().map(|r| r.key).collect();
    assert_eq!(keys, vec!["weak title and text", "title"]);
}

#[test]
fn near_tie_goes_to_the_newer() {
    // Scores 0.83 and 0.81 share bucket 16, so the newer one comes first.
    let mut rows = [(16_u32, 100_i64, "older"), (16, 200, "newer"), (15, 300, "newest")];
    rows.sort_by(order);
    let keys: Vec<&str> = rows.iter().map(|row| row.2).collect();
    assert_eq!(keys, vec!["newer", "older", "newest"]);
    let mut same = [(1_u32, 5_i64, "b"), (1, 5, "a")];
    same.sort_by(order);
    assert_eq!(same[0].2, "a", "the id breaks a full tie");
}

#[test]
fn the_path_without_bm25() {
    // With no bm25, a prompt hit (2) and an answer hit (1) give F 1 and 0.5.
    let ranked = buckets(vec![
        candidate("answer", Some(hit_score(2, None)), 0.0),
        candidate("prompt", Some(hit_score(0, None)), 0.0),
    ]);
    let order: Vec<(&str, u32)> = ranked.iter().map(|r| (r.key, r.bucket)).collect();
    assert_eq!(order, vec![("prompt", 16), ("answer", 8)]);
}
