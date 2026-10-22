//! The query language of conversation search (the plan's DQ15): what a person types,
//! turned into one FTS5 expression and the words of the title scorer.
//!
//! A client never sends FTS5 syntax, so a key never makes a syntax error. Words split on
//! white space; a `"double quoted"` part is one phrase, and an open quote at the end
//! counts as closed. Each word or phrase becomes one FTS5 string with each `"` doubled,
//! and all of them must match in one doc (`AND`). Only the last term matches as a
//! prefix, and only when the query does not end with white space and its last token
//! has [`PREFIX_CHARS`] characters or more: the index has a prefix index of that length
//! (migration 9), and a shorter prefix would merge the doclists of many terms in one
//! step that no interrupt reaches. A term whose tokens all have fewer than 2
//! characters, or that has no token, goes to the title scorer only.
//!
//! A pure module: no IO, no runtime, nothing of the crate outside it (`cargo xtask
//! tidy`).

/// The most code points of a query.
pub const MAX_CHARS: usize = 256;

/// The most words and phrases of a query.
pub const MAX_TERMS: usize = 16;

/// The shortest last token that matches as a prefix: the length of the prefix index of
/// migration 9.
pub const PREFIX_CHARS: usize = 3;

/// The shortest token that the full text index searches for.
const MIN_TOKEN_CHARS: usize = 2;

/// Why a query is not valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryError {
    /// The query has more than [`MAX_CHARS`] code points.
    TooLong {
        /// The code points of the query, after a trim.
        chars: usize,
    },
    /// The query has more than [`MAX_TERMS`] words and phrases.
    TooManyTerms {
        /// The words and phrases of the query.
        terms: usize,
    },
}

/// One word or phrase of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// The text as typed, without its quotes.
    pub text: String,
    /// Its tokens by the `unicode61` rule: the runs of letters and digits.
    pub tokens: Vec<String>,
    /// True when its last token matches as a prefix.
    pub prefix: bool,
    /// True when the full text index searches for it.
    pub indexed: bool,
}

/// A parsed query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Every word and phrase, in order. The title scorer matches each one.
    pub terms: Vec<Term>,
    /// The FTS5 expression of the terms that the index searches for; `None` when there
    /// is none, so the search is of the titles only.
    pub fts: Option<String>,
}

impl Query {
    /// True when the query has no term: the list of recent conversations.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The text of every term, for the title scorer.
    pub fn words(&self) -> Vec<&str> {
        self.terms.iter().map(|term| term.text.as_str()).collect()
    }
}

/// A character that the `unicode61` tokenizer keeps in a token.
pub fn is_token_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// The tokens of `text` by the `unicode61` rule.
pub fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !is_token_char(c))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parses what a person typed. An empty query, or one of white space, has no term.
pub fn parse(query: &str) -> Result<Query, QueryError> {
    let trimmed = query.trim();
    let chars = trimmed.chars().count();
    if chars > MAX_CHARS {
        return Err(QueryError::TooLong { chars });
    }
    let parts = split(trimmed);
    if parts.len() > MAX_TERMS {
        return Err(QueryError::TooManyTerms { terms: parts.len() });
    }
    let ends_open = !query.ends_with(char::is_whitespace);
    let last = parts.len().saturating_sub(1);
    let terms: Vec<Term> = parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let tokens = tokens(&part.text);
            let indexed = tokens.iter().any(|token| token.chars().count() >= MIN_TOKEN_CHARS);
            let prefix = index == last
                && ends_open
                && !part.closed
                && indexed
                && tokens.last().is_some_and(|token| token.chars().count() >= PREFIX_CHARS);
            Term { text: part.text, tokens, prefix, indexed }
        })
        .collect();
    let fts: Vec<String> = terms
        .iter()
        .filter(|term| term.indexed)
        .map(|term| {
            let quoted = format!("\"{}\"", term.text.replace('"', "\"\""));
            if term.prefix { quoted + "*" } else { quoted }
        })
        .collect();
    let fts = (!fts.is_empty()).then(|| fts.join(" AND "));
    Ok(Query { terms, fts })
}

/// A word or a phrase before its tokens.
struct Part {
    text: String,
    /// A phrase whose closing quote was typed.
    closed: bool,
}

/// The words and phrases of `text`: white space splits words, and a `"` starts or ends
/// a phrase, also inside a word.
fn split(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_phrase = false;
    for c in text.chars() {
        if c == '"' {
            if in_phrase {
                parts.push(Part { text: std::mem::take(&mut current), closed: true });
            } else if !current.is_empty() {
                parts.push(Part { text: std::mem::take(&mut current), closed: false });
            }
            in_phrase = !in_phrase;
        } else if c.is_whitespace() && !in_phrase {
            if !current.is_empty() {
                parts.push(Part { text: std::mem::take(&mut current), closed: false });
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        parts.push(Part { text: current, closed: false });
    }
    for part in &mut parts {
        let trimmed = part.text.trim();
        if trimmed.len() != part.text.len() {
            part.text = trimmed.to_owned();
        }
    }
    parts.retain(|part| !part.text.is_empty());
    parts
}

#[cfg(test)]
mod tests;
