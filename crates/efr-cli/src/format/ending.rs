//! The lines of the end of a turn (TM-Endings 15 to 22, TM-Legend 33 to 36): why the
//! model stopped, how the turn ended and what fixes a failure, as pieces with their
//! tones; the tokens of `efr history --verbose`; a turn's settings; and how efr paints
//! pieces as rows. `format.rs` re-exports the items that other files name.
//!
//! The outcome comes first, in its role: `✓ done` (the mark in `success`, the word in
//! `text`), `■ interrupted` (the mark in `warning`), `✗ failed` (both in `error`, bold).
//! The cost follows after two spaces, muted: the time, the gauge, the tokens out and
//! the share of the prompt cache, apart by ` · `. The gauge is muted here: the end line
//! says how full the context is, it does not warn. Under a failure, 2 columns in, the
//! reason in `text`, then the command that fixes it alone on its row in `code`.

use std::fmt::Write as _;
use std::time::Duration;

use efr_protocol::{ContextUse, StopReason, TurnStop, Usage};
use efr_render::{Glyphs, RenderOptions};

use super::route::Pieces;
use super::{Tone, context, elapsed, one_line, paint, tokens, wrap_spans};
use crate::error::Fix;

/// How much further a row of a line of efr's own goes on than its first row, when the
/// line is wider than the screen.
const GOES_ON: usize = 2;

/// How far the rows under an outcome line stand in: the reason of a failure, its fix,
/// what goes again after an interrupt.
pub(crate) const UNDER: usize = 2;

/// The smallest input of a turn, in tokens, for which the end-of-turn line shows the
/// share that the prompt cache served. Below the providers' minimums (512 tokens on
/// Claude, 1024 on OpenAI) no cache entry forms, so `cache 0%` there is only noise.
pub(crate) const CACHE_SHOWN_FROM: u64 = 2_048;

/// The outcome line of a completed turn (TM-Endings 15, TM-Legend 35): `✓ done  42s ·
/// ctx 43% · 1.1k out · cache 91%`. Each part of the cost is left out when it is not
/// known, the cache share when the input is below [`CACHE_SHOWN_FROM`]. The input
/// tokens and the size of the context are not on the line: `ctx 43%` says how full it
/// is, and `efr context` tells more.
pub(crate) fn done_line(
    took: Option<Duration>,
    usage: Option<&Usage>,
    context: Option<&ContextUse>,
    glyphs: &Glyphs,
) -> Pieces {
    let mut cost = Vec::new();
    cost.extend(shown_time(took));
    cost.extend(context.and_then(context::Gauge::of).map(context::Gauge::text));
    if let Some(usage) = usage {
        cost.push(format!("{} out", short(usage.output_tokens)));
        cost.extend(cache_share(usage));
    }
    outcome(glyphs.ok, Tone::Success, "done", Tone::Text, &cost, glyphs)
}

/// The outcome line of an interrupted turn (TM-Endings 20): `■ interrupted  12s · ctx
/// 43%`, each part of the cost left out when it is not known.
pub(crate) fn interrupted_line(
    took: Option<Duration>,
    context: Option<&ContextUse>,
    glyphs: &Glyphs,
) -> Pieces {
    let mut cost = Vec::new();
    cost.extend(shown_time(took));
    cost.extend(context.and_then(context::Gauge::of).map(context::Gauge::text));
    outcome(glyphs.interrupted, Tone::Attention, "interrupted", Tone::Text, &cost, glyphs)
}

/// The outcome line of a failed turn (TM-Endings 21, 22): `✗ failed  9s`, the mark and
/// the word in `error` bold.
pub(crate) fn failed_line(took: Option<Duration>, glyphs: &Glyphs) -> Pieces {
    let cost: Vec<String> = shown_time(took).into_iter().collect();
    outcome(glyphs.failed, Tone::FailureBold, "failed", Tone::FailureBold, &cost, glyphs)
}

/// The time of a turn on its outcome line, as the status row counts it, from 1 s, as
/// the time of a call row (TM-Calls): a turn that took less shows none.
fn shown_time(took: Option<Duration>) -> Option<String> {
    took.filter(|took| *took >= Duration::from_secs(1)).map(elapsed)
}

/// A count of tokens as [`tokens`] gives it, with no tenths when they are 0: `32k`,
/// `1.1k`, as TM-Endings 18 writes them.
fn short(count: u64) -> String {
    let text = tokens(count);
    match text.strip_suffix(".0k").or_else(|| text.strip_suffix(".0M")) {
        Some(whole) => format!("{whole}{}", if text.ends_with('k') { "k" } else { "M" }),
        None => text,
    }
}

/// The mark, the word, and the cost after two spaces, its parts apart by the separator.
fn outcome(
    mark: &str,
    marked: Tone,
    word: &str,
    worded: Tone,
    cost: &[String],
    glyphs: &Glyphs,
) -> Pieces {
    let mut line =
        vec![(mark.to_owned(), marked), (" ".to_owned(), Tone::Plain), (word.to_owned(), worded)];
    if !cost.is_empty() {
        line.push(("  ".to_owned(), Tone::Plain));
        line.push((cost.join(&format!(" {} ", glyphs.separator)), Tone::Dim));
    }
    line
}

/// `cache 91%`: the turn's cached input as a percent of its whole input, rounded down,
/// as Claude Code's hit ratio counts it, so cache writes count as misses. `None` when
/// the input is below [`CACHE_SHOWN_FROM`].
fn cache_share(usage: &Usage) -> Option<String> {
    if usage.input_tokens < CACHE_SHOWN_FROM {
        return None;
    }
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let percent = u128::from(cached) * 100 / u128::from(usage.input_tokens);
    Some(format!("cache {percent}%"))
}

/// The tokens of one turn for `efr history --verbose` (DV10), such as `18.2k in, 1.1k
/// out, cache 91%, wrote 1.2k (1h)`: the input with its cached part, the output, and
/// what the turn wrote to the prompt cache, with the part of one hour when the provider
/// says it (`wrote 1.2k (800 1h)` when only a part has the hour). The cache share
/// follows the turn line's rule ([`CACHE_SHOWN_FROM`]). It holds numbers only, never
/// text of the turn.
pub(crate) fn usage_line(usage: &Usage) -> String {
    let mut line =
        format!("{} in, {} out", tokens(usage.input_tokens), tokens(usage.output_tokens));
    if let Some(share) = cache_share(usage) {
        let _ = write!(line, ", {share}");
    }
    if usage.cache_write_tokens > 0 {
        let _ = write!(line, ", wrote {}", tokens(usage.cache_write_tokens));
        match usage.cache_write_1h_tokens {
            0 => {}
            hour if hour >= usage.cache_write_tokens => line.push_str(" (1h)"),
            hour => {
                let _ = write!(line, " ({} 1h)", tokens(hour));
            }
        }
    }
    line
}

/// Why the model of a completed turn stopped, in words, and the provider's reason:
/// `the answer reached the output limit` and `max_tokens`.
fn stop_words(stop: &TurnStop) -> (&'static str, &str) {
    let what = match stop.reason {
        _ if stop.empty => "the model stopped with no answer",
        StopReason::MaxTokens => "the answer reached the output limit",
        StopReason::Refusal => "the model refused",
        StopReason::ContentFilter => "the provider stopped the answer for its content",
        StopReason::PauseTurn => "the provider paused the turn",
        _ => "the model stopped",
    };
    (what, stop.reason.as_str())
}

/// The line before the end of a completed turn whose model gave no answer, or whose
/// answer stopped before its end (TM-Endings 18, 19, TM-Legend 33): `! stopped: the
/// answer reached the output limit (max_tokens)`. The mark and `stopped:` in
/// `warning`, the sentence in `text`, the provider's reason muted.
pub(crate) fn stop_line(stop: &TurnStop, glyphs: &Glyphs) -> Pieces {
    let (what, reason) = stop_words(stop);
    vec![
        (glyphs.stopped.to_owned(), Tone::Attention),
        (" ".to_owned(), Tone::Plain),
        ("stopped:".to_owned(), Tone::Attention),
        (format!(" {what} "), Tone::Text),
        (format!("({})", one_line(reason)), Tone::Dim),
    ]
}

/// The rows under a failed turn (TM-Endings 21, 22): the reason in `text`, then the fix
/// on a row of its own.
pub(crate) fn failure_rows(reason: &str, fix: Option<&Fix>) -> Vec<Pieces> {
    let mut rows = vec![vec![(one_line(reason), Tone::Text)]];
    rows.extend(fix.map(Fix::pieces));
    rows
}

/// The row under an interrupted turn whose unread steers go again as a new prompt
/// (TM-Endings 20): `your steer "use TurnState" goes again as a new prompt`, muted.
/// `text` is the new prompt; a prompt of several lines shows its first line and `more`.
pub(crate) fn resent_row(text: &str, more: &str) -> Pieces {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let line = match (lines.next(), lines.next()) {
        (None, _) => "your steer goes again as a new prompt".to_owned(),
        (Some(first), rest) => {
            let cut = if rest.is_some() { more } else { "" };
            format!("your steer \"{}{cut}\" goes again as a new prompt", one_line(first))
        }
    };
    vec![(line, Tone::Dim)]
}

/// `pieces` painted as rows with their newlines, the first row `indent` columns in. On
/// a terminal, a row wider than the screen goes on in the next one, [`GOES_ON`] columns
/// further in, so a line of efr's own never wraps where the terminal chooses. Off a
/// terminal the line is one plain row.
pub(crate) fn rows(pieces: &[(String, Tone)], indent: usize, options: &RenderOptions) -> String {
    let lead = " ".repeat(indent);
    let rows = match super::columns(options) {
        Some((width, method)) => {
            let first = width.saturating_sub(indent).max(1);
            let rest = width.saturating_sub(indent + GOES_ON).max(1);
            wrap_spans(pieces, first, rest, method)
        }
        None => vec![pieces.to_vec()],
    };
    let mut out = String::new();
    for (at, row) in rows.iter().enumerate() {
        out.push_str(&lead);
        if at > 0 {
            out.push_str(&" ".repeat(GOES_ON));
        }
        for (text, tone) in row {
            out.push_str(&paint(text, *tone, options));
        }
        out.push('\n');
    }
    out
}

/// The plain text of `pieces`.
#[cfg(test)]
pub(crate) fn plain(pieces: &[(String, Tone)]) -> String {
    pieces.iter().map(|(text, _)| text.as_str()).collect()
}

#[cfg(test)]
mod tests;
