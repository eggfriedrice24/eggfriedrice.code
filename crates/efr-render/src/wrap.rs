//! Word wrap of prose: a paragraph or a heading breaks at spaces into rows of the
//! width, so a word is not cut where the terminal would cut it; text inside a list
//! item or a quote wraps at the width that its indent leaves. Widths count as the
//! terminal counts them ([`WidthMethod`]), or a row can be one cell too wide and the
//! terminal wraps it again.

use unicode_segmentation::UnicodeSegmentation as _;

use crate::style::{Line, Span, Style, push_span};
use crate::width::{WidthMethod, text_width};

/// A piece of a span: a run of spaces or a run of other characters.
struct Piece<'a> {
    text: &'a str,
    style: Style,
    link: Option<&'a String>,
    width: usize,
}

/// A word (adjacent non-space pieces, possibly in several styles) or the spaces
/// before it.
enum Token<'a> {
    Gap(Vec<Piece<'a>>),
    Word(Vec<Piece<'a>>),
}

/// Breaks `spans` into lines of at most `width` columns as a terminal that counts by
/// `method` shows them, at spaces where it can and inside a word only when the word
/// alone is wider than a line, between grapheme clusters for `WidthMethod::Grapheme`.
/// Spaces at a break are dropped.
pub(crate) fn wrap(spans: &[Span], width: usize, method: WidthMethod) -> Vec<Line> {
    let width = width.max(1);
    let mut lines: Vec<Line> = Vec::new();
    let mut line: Line = Vec::new();
    let mut used = 0;
    let mut gap: Vec<Piece<'_>> = Vec::new();

    for token in tokens(spans, method) {
        match token {
            Token::Gap(pieces) => gap = pieces,
            Token::Word(pieces) => {
                let word_width: usize = pieces.iter().map(|piece| piece.width).sum();
                let gap_width: usize = gap.iter().map(|piece| piece.width).sum();
                let keep_gap = used > 0 || lines.is_empty();
                if used > 0 && used + gap_width + word_width > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                } else if keep_gap {
                    for piece in gap.drain(..) {
                        used += piece.width;
                        push_piece(&mut line, &piece, piece.text);
                    }
                }
                gap.clear();
                place_word(&pieces, width, method, &mut lines, &mut line, &mut used);
            }
        }
    }
    lines.push(line);
    lines
}

/// Appends a word, splitting it across lines when it does not fit on an empty line.
fn place_word(
    pieces: &[Piece<'_>],
    width: usize,
    method: WidthMethod,
    lines: &mut Vec<Line>,
    line: &mut Line,
    used: &mut usize,
) {
    let word_width: usize = pieces.iter().map(|piece| piece.width).sum();
    if *used + word_width <= width {
        for piece in pieces {
            push_piece(line, piece, piece.text);
            *used += piece.width;
        }
        return;
    }
    for piece in pieces {
        let mut start = 0;
        let cells: Vec<(usize, &str)> = match method {
            WidthMethod::CodePoint => piece
                .text
                .split_inclusive(|_| true)
                .scan(0, |at, cell| {
                    let start = *at;
                    *at += cell.len();
                    Some((start, cell))
                })
                .collect(),
            WidthMethod::Grapheme => piece.text.grapheme_indices(true).collect(),
        };
        for (at, cell) in cells {
            let w = text_width(cell, method);
            if *used + w > width && *used > 0 {
                push_piece(line, piece, &piece.text[start..at]);
                lines.push(std::mem::take(line));
                *used = 0;
                start = at;
            }
            *used += w;
        }
        push_piece(line, piece, &piece.text[start..]);
    }
}

fn push_piece(line: &mut Line, piece: &Piece<'_>, text: &str) {
    push_span(line, Span::linked(text, piece.style, piece.link.cloned()));
}

/// Splits spans into words and the gaps between them.
fn tokens(spans: &[Span], method: WidthMethod) -> Vec<Token<'_>> {
    let mut tokens: Vec<Token<'_>> = Vec::new();
    for span in spans {
        let mut rest = span.text.as_str();
        while !rest.is_empty() {
            let is_space = rest.starts_with(' ');
            let end = rest.find(|c: char| (c == ' ') != is_space).unwrap_or(rest.len());
            let (text, tail) = rest.split_at(end);
            rest = tail;
            let piece = Piece {
                text,
                style: span.style,
                link: span.link.as_ref(),
                width: text_width(text, method),
            };
            match (tokens.last_mut(), is_space) {
                (Some(Token::Gap(pieces)), true) | (Some(Token::Word(pieces)), false) => {
                    pieces.push(piece);
                }
                (_, true) => tokens.push(Token::Gap(vec![piece])),
                (_, false) => tokens.push(Token::Word(vec![piece])),
            }
        }
    }
    glue_citations(tokens)
}

/// `tokens` with each citation mark of web search (a word that starts with a link whose
/// text is `[1]`) joined to the word before it, with the spaces between them, so a row
/// never starts with a mark apart from the words that it backs (TM-Web 10: `Deck [3].`).
/// A source line commits whole (`renderer`), so the join is the same while it streams.
fn glue_citations(tokens: Vec<Token<'_>>) -> Vec<Token<'_>> {
    let mut out: Vec<Token<'_>> = Vec::with_capacity(tokens.len());
    for token in tokens {
        let cites = matches!(&token, Token::Word(pieces) if pieces.first().is_some_and(|piece| {
            piece.link.is_some() && is_citation(piece.text)
        }));
        let after_word = matches!(out.as_slice(), [.., Token::Word(_), Token::Gap(_)]);
        match token {
            Token::Word(pieces) if cites && after_word => {
                let gap = match out.pop() {
                    Some(Token::Gap(gap)) => gap,
                    _ => Vec::new(),
                };
                if let Some(Token::Word(word)) = out.last_mut() {
                    word.extend(gap);
                    word.extend(pieces);
                }
            }
            token => out.push(token),
        }
    }
    out
}

/// True when `text` is a citation mark: `[`, digits, `]`, as efrd writes ` [[1]](<url>)`
/// after the words that a web source backs.
pub(crate) fn is_citation(text: &str) -> bool {
    text.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests;
