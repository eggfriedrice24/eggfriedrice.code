//! The rank of conversation search (the plan's DQ21): one score per conversation from
//! its best text hit and its title, and the order of the rows.
//!
//! - `F`: the best `weight(kind) * -bm25` over the hits of the conversation, divided by
//!   the best over all conversations of the query, so it is in `[0, 1]`.
//! - `T`: the fuzzy title score against the ideal score of the query, in `[0, 1]`; 0
//!   when a word does not match the title.
//! - `S = (max(T, F) + 0.25 * min(T, F)) / 1.25`: a match in the title and in the text
//!   beats a match in one of them.
//! - The order: `floor(S * 20)` high first, then the newest conversation, then the id,
//!   so a near tie (within 0.05) goes to the newer conversation.
//! - A match in the title only with `T` below [`MIN_TITLE_SCORE`] is no row.
//!
//! A pure module: no IO, no runtime, nothing of the crate outside it (`cargo xtask
//! tidy`).

use std::cmp::Ordering;

/// The least title score of a conversation that matches in its title only.
pub const MIN_TITLE_SCORE: f64 = 0.2;

/// The share of the weaker of the two scores in `S`.
const SECOND_SHARE: f64 = 0.25;

/// The buckets of `S`: scores within one bucket are a tie that recency breaks.
const BUCKETS: f64 = 20.0;

/// The weight of a hit by the kind code of its doc: 2 for a prompt or a steer, 1.5 for
/// a command, 1 for an answer. What the person typed says most about a conversation.
pub fn weight(kind: i64) -> f64 {
    match kind {
        0 | 1 => 2.0,
        3 => 1.5,
        _ => 1.0,
    }
}

/// The text score of one hit: its weight times the negated `bm25()`, which FTS5 makes
/// lower for a better match. Without `bm25()`, the weight alone.
pub fn hit_score(kind: i64, bm25: Option<f64>) -> f64 {
    match bm25 {
        Some(bm25) => weight(kind) * (-bm25).max(0.0),
        None => weight(kind),
    }
}

/// The score `S` of a conversation from its text score `text` (`F`, in `[0, 1]`) and
/// its title score `title` (`T`, in `[0, 1]`).
pub fn score(text: f64, title: f64) -> f64 {
    let (high, low) = if text >= title { (text, title) } else { (title, text) };
    (high + SECOND_SHARE * low) / (1.0 + SECOND_SHARE)
}

/// The bucket of a score: `floor(S * 20)`.
pub fn bucket(score: f64) -> u32 {
    if score.is_nan() || score <= 0.0 {
        return 0;
    }
    (score * BUCKETS).floor().min(BUCKETS) as u32
}

/// One conversation that the search found, before its order.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<K> {
    /// The conversation.
    pub key: K,
    /// Its best text score, raw (not yet divided by the best of the query); `None` when
    /// it matched in its title only.
    pub text: Option<f64>,
    /// Its title score `T`.
    pub title: f64,
}

/// A candidate with its bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked<K> {
    /// The conversation.
    pub key: K,
    /// `floor(S * 20)`.
    pub bucket: u32,
}

/// The bucket of each candidate that makes a row, best bucket first. Within a bucket
/// the order is not decided yet: [`order`] needs the recency of each.
pub fn buckets<K>(candidates: Vec<Candidate<K>>) -> Vec<Ranked<K>> {
    let best = candidates.iter().filter_map(|candidate| candidate.text).fold(0.0_f64, f64::max);
    let mut ranked: Vec<Ranked<K>> = candidates
        .into_iter()
        .filter(|candidate| candidate.text.is_some() || candidate.title >= MIN_TITLE_SCORE)
        .map(|candidate| {
            let text = match candidate.text {
                Some(_) if best <= 0.0 => 1.0,
                Some(text) => (text / best).clamp(0.0, 1.0),
                None => 0.0,
            };
            Ranked { bucket: bucket(score(text, candidate.title)), key: candidate.key }
        })
        .collect();
    ranked.sort_by_key(|candidate| std::cmp::Reverse(candidate.bucket));
    ranked
}

/// The order of rows: the bucket high first, then the newest (`recency` high first),
/// then the key.
pub fn order<K: Ord, R: Ord>(a: &(u32, R, K), b: &(u32, R, K)) -> Ordering {
    b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2))
}

#[cfg(test)]
mod tests;
