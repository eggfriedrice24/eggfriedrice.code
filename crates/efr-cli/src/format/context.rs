//! How full the model's context is, and the compactions that make room in it: the
//! `ctx N%` gauge of the status row and the end-of-turn line, and the one muted line of
//! each compaction. The rules are in the README of `efr-conversation`, section
//! "Context", "Display".
//!
//! N is the context's tokens as a percent of its limit, rounded down: the limit is the
//! point where efrd compacts (the trigger), or the hard cap when auto compaction is off,
//! so 100% means that a compaction runs now. The gauge's colour shows the level:
//! `success` below 50%, `warning` from 50% and `error` from 90%, each in the colour of
//! the role only (`RenderOptions::tint`), so `warning` is not bold: the gauge stands in
//! every status row and must not shout. Under `NO_COLOR` only the `error` level stands
//! out, in bold, the attribute that `error` has in place of its colour.

use efr_protocol::{Compaction, CompactionTrigger, ContextUse};
use efr_render::{RenderOptions, Role, render_trace, text_width};

use super::{Tone, paint, wrap_spans};

/// The percent from which the gauge is in the `warning` role.
const WARNING_AT: u64 = 50;

/// The percent from which the gauge is in the `error` role.
const ERROR_AT: u64 = 90;

/// The gauge of `ctx N%`: the context's tokens as a percent of its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gauge {
    percent: u64,
}

impl Gauge {
    /// The gauge of `context`; `None` without a limit, which no daemon sends.
    pub(crate) fn of(context: &ContextUse) -> Option<Gauge> {
        let percent = u128::from(context.tokens) * 100 / u128::from(context.limit).max(1);
        (context.limit > 0).then(|| Gauge { percent: u64::try_from(percent).unwrap_or(u64::MAX) })
    }

    /// `ctx 43%`.
    pub(crate) fn text(self) -> String {
        format!("ctx {}%", self.percent)
    }

    /// The role of its level.
    pub(crate) fn role(self) -> Role {
        match self.percent {
            ..WARNING_AT => Role::Success,
            WARNING_AT..ERROR_AT => Role::Warning,
            _ => Role::Error,
        }
    }

    /// `ctx 43%` in the colour of its level, without the role's bold.
    pub(crate) fn paint(self, options: &RenderOptions) -> String {
        options.tint(self.role(), &self.text())
    }
}

/// A count of tokens as the context lines show it: as it is under 1000, `3.2k` under
/// 10000, then whole thousands (`24k`, `207k`) and `1.2M`. The digits are cut, never
/// rounded up.
pub(crate) fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..10_000 => format!("{}.{}k", tokens / 1_000, tokens / 100 % 10),
        10_000..1_000_000 => format!("{}k", tokens / 1_000),
        _ => format!("{}.{}M", tokens / 1_000_000, tokens / 100_000 % 10),
    }
}

/// `ctx 43% (89k/207k)` for the end of a turn, as a gauge and the plain rest.
pub(crate) fn end_part(context: &ContextUse) -> Option<(Gauge, String)> {
    let gauge = Gauge::of(context)?;
    Some((gauge, format!(" ({}/{})", count(context.tokens), count(context.limit))))
}

/// A muted line with the gauge in it, in the colour of its level: the end-of-turn line,
/// such as `done in 42s, ctx 43% (89k/207k), 1.1k out`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GaugedLine {
    before: String,
    gauge: Option<Gauge>,
    after: String,
}

impl GaugedLine {
    /// The line `before`, the gauge, then `after`.
    pub(crate) fn new(before: String, gauge: Option<Gauge>, after: String) -> GaugedLine {
        GaugedLine { before, gauge, after }
    }

    /// The line as plain text.
    pub(crate) fn plain(&self) -> String {
        let gauge = self.gauge.map(Gauge::text).unwrap_or_default();
        format!("{}{gauge}{}", self.before, self.after)
    }

    /// The line with its newline: muted, with the gauge in its colour, on a terminal
    /// where it fits; otherwise as a trace line, which is plain off a terminal and
    /// muted and cut to the width on one.
    pub(crate) fn render(&self, options: &RenderOptions) -> String {
        let plain = self.plain();
        let fits = text_width(&plain, options.width_method()) <= usize::from(options.width());
        let Some(gauge) = self.gauge.filter(|_| options.is_terminal() && fits) else {
            return render_trace(&plain, options);
        };
        let mut out = String::new();
        if !self.before.is_empty() {
            out.push_str(&options.paint(Role::Muted, &self.before));
        }
        out.push_str(&gauge.paint(options));
        if !self.after.is_empty() {
            out.push_str(&options.paint(Role::Muted, &self.after));
        }
        out.push('\n');
        out
    }
}

impl From<String> for GaugedLine {
    fn from(line: String) -> GaugedLine {
        GaugedLine { before: line, gauge: None, after: String::new() }
    }
}

/// The context after `compaction`, as the status row's gauge shows it until the next
/// count.
pub(crate) fn after(compaction: &Compaction) -> ContextUse {
    ContextUse {
        tokens: compaction.tokens_after,
        limit: compaction.limit,
        window: compaction.window,
    }
}

/// The one muted line of a compaction, in the scrollback and in `efr history`:
///
/// - `context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k`, with
///   `(efr compact)` for a manual one and `pruned 12 outputs` in place of the summary
///   when pruning alone made room, and `(left out 2 turns and 40 messages)` after the
///   summary for what it never saw, because it did not fit;
/// - `context full: the request was 281k of 272k tokens; compacted and retried` for an
///   overflow, with the same note of what the summary never saw;
/// - `context full: compaction did not free enough room (still 240k); run ,compact or
///   efr new` when the context is still at or above its limit after it, a miss of the
///   breaker. After a manual compaction only `efr new` helps.
pub(crate) fn compacted(compaction: &Compaction) -> String {
    let manual = compaction.trigger == CompactionTrigger::Manual;
    if compaction.limit > 0 && compaction.tokens_after >= compaction.limit {
        let way_out = if manual { "run efr new" } else { "run ,compact or efr new" };
        return format!(
            "context full: compaction did not free enough room (still {}); {way_out}",
            count(compaction.tokens_after)
        );
    }
    if compaction.trigger == CompactionTrigger::Overflow {
        return format!(
            "context full: the request was {} of {} tokens; compacted and retried{}",
            count(compaction.tokens_before),
            count(compaction.window),
            left_out(compaction)
        );
    }
    let by = if manual { "efr compact" } else { "auto" };
    let mut line = format!(
        "context compacted ({by}): {} -> {} tokens",
        count(compaction.tokens_before),
        count(compaction.tokens_after)
    );
    match compaction.kept_turns {
        0 => {}
        1 => line.push_str(", kept 1 turn"),
        turns => line.push_str(&format!(", kept {turns} turns")),
    }
    match (&compaction.summary, compaction.pruned_outputs) {
        (Some(summary), _) => {
            let size = compaction
                .usage
                .map(|usage| usage.output_tokens)
                .filter(|tokens| *tokens > 0)
                .unwrap_or_else(|| estimate(summary));
            line.push_str(&format!(", summary {}", count(size)));
        }
        (None, 0) => {}
        (None, 1) => line.push_str(", pruned 1 output"),
        (None, outputs) => line.push_str(&format!(", pruned {outputs} outputs")),
    }
    line.push_str(&left_out(compaction));
    line
}

/// What the summary of `compaction` never saw, because it did not fit, such as
/// ` (left out 2 turns and 40 messages)`; empty when it saw everything.
fn left_out(compaction: &Compaction) -> String {
    let parts: Vec<String> =
        [(compaction.omitted_turns, "turn"), (compaction.omitted_messages, "message")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") })
            .collect();
    if parts.is_empty() { String::new() } else { format!(" (left out {})", parts.join(" and ")) }
}

/// The line of a compaction (see [`compacted`]) with its newline: muted on a terminal,
/// where a line wider than the screen goes on in the next row, indented 2 columns, so
/// its way out is never cut off; plain elsewhere.
pub(crate) fn compacted_rows(compaction: &Compaction, options: &RenderOptions) -> String {
    let line = compacted(compaction);
    if !options.is_terminal() {
        return format!("{line}\n");
    }
    let columns = usize::from(options.width()).max(3);
    let rows = wrap_spans(&[(line, Tone::Dim)], columns, columns - 2, options.width_method());
    let mut out = String::new();
    for (at, row) in rows.iter().enumerate() {
        if at > 0 {
            out.push_str("  ");
        }
        for (text, tone) in row {
            out.push_str(&paint(text, *tone, options));
        }
        out.push('\n');
    }
    out
}

/// The tokens of `text` as efrd estimates them without a count: 4 bytes a token,
/// rounded up.
fn estimate(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
