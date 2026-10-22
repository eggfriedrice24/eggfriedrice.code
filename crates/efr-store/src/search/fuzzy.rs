//! The fuzzy title scorer of conversation search (the plan's DQ22), after fzf's first
//! algorithm.
//!
//! Each word of a query matches on its own, as a subsequence of the title, and every
//! word must match. For one word, a forward scan finds the first place where the whole
//! word has matched, and a backward scan from there finds the shortest window that holds
//! it; the score of that window gives a point for each match, takes points for a gap,
//! and gives a bonus to a match at the start of a word, after a delimiter, at a camel
//! case step and in a run of matches, and twice the bonus to the first character of the
//! word. Smart case: the match ignores case unless the word has an upper case letter.
//!
//! The rule and its numbers are those of fzf 0.73.1, `src/algo/algo.go`: the constants
//! at lines 112 to 151 (`scoreMatch` to `initialCharClass`) and `delimiterChars` at line
//! 93, `bonusFor` at lines 264 to 292, `calculateScore` at lines 647 to 704 and
//! `FuzzyMatchV1` from line 707. No code is copied; this is the same rule in Rust. Not
//! taken: fzf's normalisation of Latin letters with diacritics and its second
//! algorithm.
//!
//! A pure module: no IO, no runtime, nothing of the crate outside it (`cargo xtask
//! tidy`).

use std::ops::Range;

/// A point for each character that matches.
const SCORE_MATCH: i32 = 16;
/// The cost of the first character of a gap.
const SCORE_GAP_START: i32 = -3;
/// The cost of each further character of a gap.
const SCORE_GAP_EXTENSION: i32 = -1;
/// A match at the start of a word.
const BONUS_BOUNDARY: i32 = SCORE_MATCH / 2;
/// A match of a character that is not a letter or a digit.
const BONUS_NON_WORD: i32 = SCORE_MATCH / 2;
/// A match at a camel case step or at the first digit after a letter.
const BONUS_CAMEL_123: i32 = BONUS_BOUNDARY + SCORE_GAP_EXTENSION;
/// The least bonus of a match that continues a run of matches.
const BONUS_CONSECUTIVE: i32 = -(SCORE_GAP_START + SCORE_GAP_EXTENSION);
/// The first character of a word counts its bonus this many times.
const BONUS_FIRST_CHAR_MULTIPLIER: i32 = 2;
/// A match at the start of a word after white space or at the start of the text.
const BONUS_BOUNDARY_WHITE: i32 = BONUS_BOUNDARY + 2;
/// A match at the start of a word after a delimiter.
const BONUS_BOUNDARY_DELIMITER: i32 = BONUS_BOUNDARY + 1;
/// The characters that delimit the words of a path or a list.
const DELIMITERS: &str = "/,:;|";

/// The class of a character, in fzf's order: a class after `NonWord` starts a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    White,
    NonWord,
    Delimiter,
    Lower,
    Upper,
    Letter,
    Number,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::White
    } else if DELIMITERS.contains(c) {
        Class::Delimiter
    } else if c.is_lowercase() {
        Class::Lower
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_numeric() {
        Class::Number
    } else if c.is_alphabetic() {
        Class::Letter
    } else {
        Class::NonWord
    }
}

/// The bonus of a match of a character of class `class` after one of class `prev`.
fn bonus(prev: Class, class: Class) -> i32 {
    if class > Class::NonWord {
        match prev {
            Class::White => return BONUS_BOUNDARY_WHITE,
            Class::Delimiter => return BONUS_BOUNDARY_DELIMITER,
            Class::NonWord => return BONUS_BOUNDARY,
            _ => {}
        }
    }
    if prev == Class::Lower && class == Class::Upper
        || prev != Class::Number && class == Class::Number
    {
        return BONUS_CAMEL_123;
    }
    match class {
        Class::NonWord | Class::Delimiter => BONUS_NON_WORD,
        Class::White => BONUS_BOUNDARY_WHITE,
        _ => 0,
    }
}

/// How a title matched the words of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// The sum of the scores of the words.
    pub score: i32,
    /// The matched code points of the title, sorted, with no overlap, and adjacent
    /// ranges merged.
    pub ranges: Vec<Range<u32>>,
}

/// One word, ready to match.
#[derive(Debug, Clone)]
struct Pattern {
    chars: Vec<char>,
    case_sensitive: bool,
}

impl Pattern {
    fn new(word: &str) -> Self {
        let case_sensitive = word.chars().any(char::is_uppercase);
        let chars =
            if case_sensitive { word.chars().collect() } else { word.chars().map(lower).collect() };
        Pattern { chars, case_sensitive }
    }

    fn fold(&self, c: char) -> char {
        if self.case_sensitive { c } else { lower(c) }
    }
}

/// The lower case of `c` as one character, as fzf folds it.
fn lower(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

/// The words of a query, ready to score titles.
#[derive(Debug, Clone)]
pub struct Scorer {
    patterns: Vec<Pattern>,
    ideal: i32,
}

impl Scorer {
    /// A scorer of `words`. A word that is empty matches every title with no score.
    pub fn new(words: &[&str]) -> Self {
        let patterns: Vec<Pattern> = words.iter().map(|word| Pattern::new(word)).collect();
        let ideal = patterns
            .iter()
            .filter_map(|pattern| {
                let own: Vec<char> = pattern.chars.clone();
                one(&own, pattern).map(|(score, _)| score)
            })
            .sum();
        Scorer { patterns, ideal }
    }

    /// The sum of the scores of each word matched against itself: the best score that a
    /// title can get.
    pub fn ideal(&self) -> i32 {
        self.ideal
    }

    /// How `title` matches every word; `None` when a word does not match.
    pub fn score(&self, title: &str) -> Option<FuzzyMatch> {
        if self.patterns.is_empty() {
            return None;
        }
        let text: Vec<char> = title.chars().collect();
        let mut score = 0;
        let mut positions = Vec::new();
        for pattern in &self.patterns {
            let (one_score, mut found) = one(&text, pattern)?;
            score += one_score;
            positions.append(&mut found);
        }
        Some(FuzzyMatch { score, ranges: ranges(positions) })
    }

    /// The score of `title` against the ideal score, from 0 to 1; 0 when a word does
    /// not match.
    pub fn relative(&self, found: &FuzzyMatch) -> f64 {
        if self.ideal <= 0 {
            return 0.0;
        }
        (f64::from(found.score) / f64::from(self.ideal)).clamp(0.0, 1.0)
    }
}

/// The score and the matched positions of one word in `text`.
fn one(text: &[char], pattern: &Pattern) -> Option<(i32, Vec<usize>)> {
    let wanted = &pattern.chars;
    if wanted.is_empty() {
        return Some((0, Vec::new()));
    }
    // The forward scan: the end of the first subsequence match.
    let mut at = 0;
    let mut end = None;
    for (index, &c) in text.iter().enumerate() {
        if pattern.fold(c) == wanted[at] {
            at += 1;
            if at == wanted.len() {
                end = Some(index + 1);
                break;
            }
        }
    }
    let end = end?;
    // The backward scan: the latest start of a match that ends there.
    let mut at = wanted.len() - 1;
    let mut start = 0;
    for index in (0..end).rev() {
        if pattern.fold(text[index]) == wanted[at] {
            if at == 0 {
                start = index;
                break;
            }
            at -= 1;
        }
    }
    Some(window_score(text, pattern, start, end))
}

/// The score of the match of `pattern` in `text[start..end]`, and its positions.
fn window_score(text: &[char], pattern: &Pattern, start: usize, end: usize) -> (i32, Vec<usize>) {
    let wanted = &pattern.chars;
    let mut positions = Vec::with_capacity(wanted.len());
    let mut score = 0;
    let mut at = 0;
    let mut in_gap = false;
    let mut run = 0;
    let mut first_bonus = 0;
    let mut prev = if start > 0 { class_of(text[start - 1]) } else { Class::White };
    for (index, &c) in text.iter().enumerate().take(end).skip(start) {
        let class = class_of(c);
        if at < wanted.len() && pattern.fold(c) == wanted[at] {
            positions.push(index);
            score += SCORE_MATCH;
            let mut this = bonus(prev, class);
            if run == 0 {
                first_bonus = this;
            } else {
                // A word start inside a run lifts the bonus of the rest of the run.
                if this >= BONUS_BOUNDARY && this > first_bonus {
                    first_bonus = this;
                }
                this = this.max(first_bonus).max(BONUS_CONSECUTIVE);
            }
            score += if at == 0 { this * BONUS_FIRST_CHAR_MULTIPLIER } else { this };
            in_gap = false;
            run += 1;
            at += 1;
        } else {
            score += if in_gap { SCORE_GAP_EXTENSION } else { SCORE_GAP_START };
            in_gap = true;
            run = 0;
            first_bonus = 0;
        }
        prev = class;
    }
    (score, positions)
}

/// Positions as sorted ranges with no overlap, adjacent ones merged.
fn ranges(mut positions: Vec<usize>) -> Vec<Range<u32>> {
    positions.sort_unstable();
    positions.dedup();
    let mut out: Vec<Range<u32>> = Vec::new();
    for position in positions {
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        match out.last_mut() {
            Some(last) if last.end == position => last.end = position.saturating_add(1),
            _ => out.push(position..position.saturating_add(1)),
        }
    }
    out
}

#[cfg(test)]
mod tests;
