//! The key of the file that a mistyped key most likely meant, for the hint of
//! [`ConfigError::UnknownKey`](crate::ConfigError::UnknownKey).
//!
//! A typo is a letter added, lost or changed, or two neighbour letters swapped. A key
//! of fewer than eight letters may be one edit away, a longer one two. A short key with
//! two edits is often another word, not a typo.

use crate::keys::keys;

/// The key of the file closest to `key`, at most the edits above away, or `None` when
/// no key is that close or `key` is a key of the file. Of keys at the same distance,
/// the first in the order of the file wins.
pub(crate) fn closest_key(key: &str) -> Option<String> {
    let most = if key.chars().count() < 8 { 1 } else { 2 };
    keys()
        .into_iter()
        .map(|known| (edits(key, &known), known))
        .filter(|(distance, _)| (1..=most).contains(distance))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, known)| known)
}

/// How many edits make `a` into `b`: letters added, lost or changed, and neighbour
/// letters swapped (the optimal string alignment distance).
fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // NOTE: three rows of the table are enough: a swap looks back two rows.
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1).min(current[j - 1] + 1).min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests;
