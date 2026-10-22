//! The snippets of conversation search (the plan's DQ18): the line of a hit that shows
//! in the list, and the lines of the preview, cut to a width.
//!
//! The list builds no FTS5 statement per row. It reads the text of the best hit of
//! each row by its sequence number and finds the words here: it folds the text and the
//! words as the `unicode61 remove_diacritics 2` tokenizer does (lower case, the
//! diacritics of Latin-1 and Latin Extended-A removed), matches each word or phrase
//! token by token (the last token of the last word as a prefix when the query asks),
//! takes the first line with the most words, and cuts it to [`LINE_CHARS`] code
//! points around its first match. A rare difference with the tokenizer costs only a
//! highlight.
//!
//! A pure module: no IO, no runtime, nothing of the crate outside it (`cargo xtask
//! tidy`).

use std::ops::Range;

/// The most code points of one line of a snippet or a preview.
pub const LINE_CHARS: usize = 400;

/// The most lines of one hit of the preview.
pub const PREVIEW_LINES: usize = 12;

/// What ends or starts a line that was cut.
pub const ELLIPSIS: &str = "...";

/// The code points before the first match that a cut line keeps, when it cannot start
/// at the start of the line.
const LEAD: u32 = 40;

/// One word or phrase to find: its folded tokens, the last one as a prefix when
/// `prefix` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    tokens: Vec<String>,
    prefix: bool,
}

impl Needle {
    /// A needle of the tokens of a word or phrase.
    pub fn new(tokens: &[String], prefix: bool) -> Self {
        Needle {
            tokens: tokens.iter().map(|token| token.chars().map(fold).collect()).collect(),
            prefix,
        }
    }

    /// False when `line`, which is ASCII, surely holds no match: its first token does
    /// not occur in it, in any case. A needle that is not ASCII may match anywhere.
    fn may_match_ascii(&self, line: &[u8]) -> bool {
        let Some(first) = self.tokens.first().filter(|token| token.is_ascii()) else {
            return true;
        };
        let first = first.as_bytes();
        !first.is_empty()
            && line.windows(first.len()).any(|window| window.eq_ignore_ascii_case(first))
    }
}

/// `c` as the tokenizer compares it: lower case, without the diacritic of a Latin
/// letter.
pub fn fold(c: char) -> char {
    let lower =
        if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) };
    match lower {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// One token of a text: its code points, and its bytes in the folded text.
struct Token {
    start: u32,
    end: u32,
    folded: Range<usize>,
}

/// The tokens of a text, with their folded forms in one buffer, so a long text costs
/// two allocations and not one per token.
struct Tokens {
    folded: String,
    tokens: Vec<Token>,
}

impl Tokens {
    fn of(text: &str) -> Self {
        let mut folded = String::with_capacity(text.len());
        let mut tokens = Vec::new();
        let mut current: Option<Token> = None;
        for (at, c) in text.chars().enumerate() {
            let at = u32::try_from(at).unwrap_or(u32::MAX);
            if c.is_alphanumeric() {
                let from = folded.len();
                let token = current.get_or_insert(Token { start: at, end: at, folded: from..from });
                folded.push(fold(c));
                token.end = at.saturating_add(1);
                token.folded.end = folded.len();
            } else if let Some(token) = current.take() {
                tokens.push(token);
            }
        }
        tokens.extend(current);
        Tokens { folded, tokens }
    }

    fn text(&self, index: usize) -> &str {
        &self.folded[self.tokens[index].folded.clone()]
    }
}

/// Each place in `text` where a needle matches, as (the needle's index, the code
/// points from its first token to its last), in text order.
pub fn find(text: &str, needles: &[Needle]) -> Vec<(usize, Range<u32>)> {
    let all = Tokens::of(text);
    let tokens = &all.tokens;
    let mut found = Vec::new();
    for start in 0..tokens.len() {
        for (index, needle) in needles.iter().enumerate() {
            let n = needle.tokens.len();
            if n == 0 || start + n > tokens.len() {
                continue;
            }
            let matches = needle.tokens.iter().enumerate().all(|(k, wanted)| {
                let have = all.text(start + k);
                if k + 1 == n && needle.prefix {
                    have.starts_with(wanted.as_str())
                } else {
                    have == wanted
                }
            });
            if matches {
                found.push((index, tokens[start].start..tokens[start + n - 1].end));
            }
        }
    }
    found
}

/// The snippet of a hit: the first line of `text` with the most needles, cut to
/// [`LINE_CHARS`] around its first match, and the ranges of the matches in it. With no
/// match, the first line that is not blank, with no range.
pub fn snippet(text: &str, needles: &[Needle]) -> (String, Vec<Range<u32>>) {
    let mut best: Option<(usize, &str, Vec<Range<u32>>)> = None;
    for line in text.split('\n') {
        // A cheap look first: most lines of a long doc hold no needle.
        if line.is_ascii() && !needles.iter().any(|needle| needle.may_match_ascii(line.as_bytes()))
        {
            continue;
        }
        let found = find(line, needles);
        if found.is_empty() {
            continue;
        }
        let mut kinds: Vec<usize> = found.iter().map(|(index, _)| *index).collect();
        kinds.sort_unstable();
        kinds.dedup();
        if best.as_ref().is_none_or(|(count, _, _)| kinds.len() > *count) {
            let ranges = merge(found.into_iter().map(|(_, range)| range).collect());
            best = Some((kinds.len(), line, ranges));
            if kinds.len() == needles.len() {
                // No later line can hold more needles.
                break;
            }
        }
    }
    match best {
        Some((_, line, ranges)) => cut(line, &ranges, LINE_CHARS),
        None => {
            let line = text.split('\n').find(|line| !line.trim().is_empty()).unwrap_or("");
            cut(line, &[], LINE_CHARS)
        }
    }
}

/// Sorted ranges with no overlap, adjacent ones merged.
fn merge(mut ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut out: Vec<Range<u32>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

/// `line` cut to at most `max` code points around its first range, with
/// [`ELLIPSIS`] where it was cut, and the ranges moved to the cut text. A line that
/// fits stays as it is.
pub fn cut(line: &str, ranges: &[Range<u32>], max: usize) -> (String, Vec<Range<u32>>) {
    let chars: Vec<char> = line.chars().collect();
    let len = u32::try_from(chars.len()).unwrap_or(u32::MAX);
    let max = u32::try_from(max).unwrap_or(u32::MAX);
    if len <= max {
        let ranges = ranges
            .iter()
            .map(|r| r.start.min(len)..r.end.min(len))
            .filter(|r| r.start < r.end)
            .collect();
        return (line.to_owned(), ranges);
    }
    let dots = ELLIPSIS.len() as u32;
    if max <= 4 * dots {
        // Too narrow for a cut with marks: the start of the line.
        let ranges = ranges
            .iter()
            .map(|r| r.start.min(max)..r.end.min(max))
            .filter(|r| r.start < r.end)
            .collect();
        return (chars[..max as usize].iter().collect(), ranges);
    }
    let first = ranges.first().map_or(0, |range| range.start.min(len));
    // Start at the start of the line when the first match fits in the first part.
    let (start, lead) = if first + LEAD.min(max / 2) < max.saturating_sub(dots) {
        (0, 0)
    } else {
        (first.saturating_sub(LEAD), dots)
    };
    let room = max.saturating_sub(lead);
    let (end, tail) = if start + room >= len { (len, 0) } else { (start + room - dots, dots) };
    let mut text = String::with_capacity((end - start + lead + tail) as usize * 2);
    if lead > 0 {
        text.push_str(ELLIPSIS);
    }
    text.extend(&chars[start as usize..end as usize]);
    if tail > 0 {
        text.push_str(ELLIPSIS);
    }
    let moved = ranges
        .iter()
        .map(|r| r.start.max(start).min(end)..r.end.max(start).min(end))
        .filter(|r| r.start < r.end)
        .map(|r| r.start - start + lead..r.end - start + lead)
        .collect();
    (text, moved)
}

/// The lines of a preview hit: each line with a match, with one line of context before
/// and after it, at most [`PREVIEW_LINES`] in order, each cut to [`LINE_CHARS`]. With
/// no match, the first lines.
pub fn preview(lines: Vec<(String, Vec<Range<u32>>)>) -> Vec<(String, Vec<Range<u32>>)> {
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, ranges))| !ranges.is_empty())
        .map(|(i, _)| i)
        .collect();
    let mut keep = vec![false; lines.len()];
    if hits.is_empty() {
        keep.iter_mut().take(PREVIEW_LINES).for_each(|k| *k = true);
    } else {
        let mut kept = 0;
        // The lines with a match first, then their context, so the context never takes
        // the room of a match.
        let context = hits.iter().flat_map(|&hit| [hit.checked_sub(1), Some(hit + 1)]).flatten();
        for at in hits.iter().copied().chain(context) {
            if kept < PREVIEW_LINES && at < keep.len() && !keep[at] {
                keep[at] = true;
                kept += 1;
            }
        }
    }
    lines
        .into_iter()
        .zip(keep)
        .filter(|(_, keep)| *keep)
        .map(|((line, ranges), _)| cut(&line, &ranges, LINE_CHARS))
        .collect()
}

#[cfg(test)]
mod tests;
