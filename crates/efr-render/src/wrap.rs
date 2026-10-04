//! Hard wrapping for indented blocks. Prose at the top level is never wrapped, so
//! the terminal reflows it on resize; text inside a list item or a quote is wrapped
//! here, so its continuation lines keep the indent.

use unicode_width::UnicodeWidthChar as _;

use crate::style::{Line, Span, Style, push_span};

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

/// Breaks `spans` into lines of at most `width` columns, at spaces where it can and
/// inside a word only when the word alone is wider than a line. Spaces at a break
/// are dropped.
pub(crate) fn wrap(spans: &[Span], width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut lines: Vec<Line> = Vec::new();
    let mut line: Line = Vec::new();
    let mut used = 0;
    let mut gap: Vec<Piece<'_>> = Vec::new();

    for token in tokens(spans) {
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
                place_word(&pieces, width, &mut lines, &mut line, &mut used);
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
        for (at, c) in piece.text.char_indices() {
            let w = c.width().unwrap_or(0);
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
fn tokens(spans: &[Span]) -> Vec<Token<'_>> {
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
                width: text.chars().map(|c| c.width().unwrap_or(0)).sum(),
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
    tokens
}

#[cfg(test)]
mod tests;
