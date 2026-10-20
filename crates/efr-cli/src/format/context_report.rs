//! The lines of `efr context`: what fills the model's context of a conversation, from
//! `conversation.context`.
//!
//! The provider's own count stands first, with the `ctx N%` gauge of the status row.
//! Every other size is efrd's estimate (the bytes of the request divided by 4), so the
//! parts say `about`. The lines never hold the text of a message: only sizes, counts
//! and the labels that `efr history` shows already. A label comes from the daemon, so
//! it is printed through [`one_line`], and on a terminal a line is cut to the width.
//!
//! ```text
//! model        claude-opus-5-5, window 1.0M, compacts at 760k (76%)
//! counted      98.4k at the end of the last turn, 3m 0s ago, ctx 12%
//! estimate     about 96.7k for the next request, without its prompt
//!   system          6.1k
//!   tools           4.8k  9 tools
//!   prompts         2.0k  14 prompts
//!   preambles       9.8k  14 preambles; only the newest is current
//!   tool outputs   48.3k  28 outputs
//! largest          12.3k  output of $ cargo test, turn 12
//!                   8.0k  output of read crates/efr-daemon/src/run.rs, turn 2
//! compactions  1, the newest 2h 5m ago (auto): 231k -> 24.0k with a summary
//! cache        last turn 91%; conversation 83% read, 210k written (200k for 1h)
//! ```

use std::fmt::Write as _;

use efr_protocol::{
    CompactionTrigger, ContextItem, ContextPart, ContextPartKind, ContextUse,
    ConversationContextResult, Usage,
};
use efr_render::{RenderOptions, text_width};
use jiff::Timestamp;

use super::context::Gauge;
use super::{CACHE_SHOWN_FROM, Tone, ago, cut, one_line, paint, tokens};

/// The width of the column of the names.
const KEY: usize = 13;

/// The width of the column of the names of the parts, after their indent.
const PART: usize = 14;

/// The width of the tokens of a large item, so that they end where the tokens of a
/// part end.
const LARGEST: usize = 2 + PART + 6 - KEY;

/// The lines of `result` at `now`, with a final newline.
pub(crate) fn lines(
    result: &ConversationContextResult,
    now: Timestamp,
    options: &RenderOptions,
) -> String {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::plain(row("model", &model(result))));
    let gauge = |tokens: u64| {
        let limit = if result.auto { result.trigger } else { result.hard_cap };
        Gauge::of(&ContextUse { tokens, limit, window: result.window })
    };
    let counted = match &result.counted {
        Some(counted) => Line::gauged(
            row(
                "counted",
                &format!(
                    "{} at the end of the last turn, {}",
                    tokens(counted.tokens),
                    ago(counted.at, now)
                ),
            ),
            gauge(counted.tokens),
        ),
        None if result.compactions.count > 0 => Line::plain(row(
            "counted",
            "none: no turn has ended with a count since the newest compaction",
        )),
        None => Line::plain(row("counted", "none: no turn has ended with a count yet")),
    };
    lines.push(counted);
    let mut estimate =
        format!("about {} for the next request, without its prompt", tokens(result.estimate));
    if result.running {
        estimate.push_str(", with the running turn so far");
    }
    let estimate_gauge = if result.counted.is_none() { gauge(result.estimate) } else { None };
    lines.push(Line::gauged(row("estimate", &estimate), estimate_gauge));
    lines.extend(result.parts.iter().map(part_row));
    for (index, item) in result.largest.iter().enumerate() {
        let key = if index == 0 { "largest" } else { "" };
        lines.push(Line::plain(row(
            key,
            &format!("{:>LARGEST$}  {}", tokens(item.tokens), item_words(item)),
        )));
    }
    if let Some(line) = compactions(result, now) {
        lines.push(Line::plain(row("compactions", &line)));
    }
    lines.push(Line::plain(row("cache", &cache(result))));
    lines.iter().map(|line| line.render(options)).collect()
}

/// One line: its plain text, and a muted end or a gauge after it on a terminal.
struct Line {
    text: String,
    muted: Option<String>,
    gauge: Option<Gauge>,
}

impl Line {
    fn plain(text: String) -> Self {
        Line { text, muted: None, gauge: None }
    }

    fn gauged(text: String, gauge: Option<Gauge>) -> Self {
        Line { text, muted: None, gauge }
    }

    fn whole(&self) -> String {
        let mut whole = self.text.clone();
        if let Some(muted) = &self.muted {
            let _ = write!(whole, "  {muted}");
        }
        if let Some(gauge) = self.gauge {
            let _ = write!(whole, ", {}", gauge.text());
        }
        whole
    }

    /// The line with its newline: off a terminal as it is; on one cut to the width,
    /// and with its muted end and its gauge in their colours when it fits.
    fn render(&self, options: &RenderOptions) -> String {
        let whole = self.whole();
        if !options.is_terminal() {
            return format!("{whole}\n");
        }
        let width = usize::from(options.width());
        if text_width(&whole, options.width_method()) > width {
            return format!("{}\n", cut(&whole, width, options.width_method()));
        }
        let mut out = self.text.clone();
        if let Some(muted) = &self.muted {
            let _ = write!(out, "  {}", paint(muted, Tone::Dim, options));
        }
        if let Some(gauge) = self.gauge {
            let _ = write!(out, ", {}", gauge.paint(options));
        }
        out.push('\n');
        out
    }
}

/// `name` in the column of the names, then `text`.
fn row(name: &str, text: &str) -> String {
    format!("{name:<KEY$}{text}")
}

/// The model and its limits: `claude-opus-5-5, window 1.0M, compacts at 760k (76%)`.
fn model(result: &ConversationContextResult) -> String {
    let mut text = format!("{}, window {}", one_line(&result.model), tokens(result.window));
    if !result.window_known {
        text.push_str(" (efr does not know the window of this model and counts with this one)");
    }
    if result.auto {
        let percent = u128::from(result.trigger) * 100 / u128::from(result.window).max(1);
        let _ = write!(text, ", compacts at {} ({percent}%)", tokens(result.trigger));
    } else {
        let _ = write!(text, ", auto compaction off, cap {}", tokens(result.hard_cap));
    }
    text
}

/// One part: its name, its estimated tokens and its count in words, muted on a terminal.
fn part_row(part: &ContextPart) -> Line {
    let name = match part.kind {
        ContextPartKind::System => "system",
        ContextPartKind::Tools => "tools",
        ContextPartKind::Summary => "summary",
        ContextPartKind::Prompts => "prompts",
        ContextPartKind::Preambles => "preambles",
        ContextPartKind::Notices => "notices",
        ContextPartKind::Answers => "answers",
        ContextPartKind::ToolCalls => "tool calls",
        ContextPartKind::ToolOutputs => "tool outputs",
        ContextPartKind::Pruned => "pruned",
        _ => "other",
    };
    let count = part.count;
    let words = match part.kind {
        ContextPartKind::Tools => counted(count, "tool"),
        ContextPartKind::Summary => counted(count, "message"),
        ContextPartKind::Prompts => counted(count, "prompt"),
        ContextPartKind::Preambles if count > 1 => {
            format!("{count} preambles; only the newest is current")
        }
        ContextPartKind::Preambles => counted(count, "preamble"),
        ContextPartKind::Notices => counted(count, "notice"),
        ContextPartKind::Answers => counted(count, "answer"),
        ContextPartKind::ToolCalls => counted(count, "call"),
        ContextPartKind::ToolOutputs => counted(count, "output"),
        ContextPartKind::Pruned => format!("{} replaced with a stub", counted(count, "output")),
        _ => String::new(),
    };
    let head = format!("  {name:<PART$}{:>6}", tokens(part.tokens));
    let muted = (!words.is_empty() && count > 0).then_some(words);
    Line { text: head, muted, gauge: None }
}

/// `count` and `what`, with an `s` for more than one.
fn counted(count: u32, what: &str) -> String {
    if count == 1 { format!("1 {what}") } else { format!("{count} {what}s") }
}

/// What a large item is, in words: `output of $ cargo test, turn 12`.
fn item_words(item: &ContextItem) -> String {
    let call = || {
        let tool = item.tool.as_deref().map(one_line);
        let label = item.label.as_deref().map(one_line);
        match (tool.as_deref(), label) {
            (Some("shell"), Some(label)) => format!("$ {label}"),
            (Some("read_file"), Some(label)) => format!("read {label}"),
            (Some("write_file"), Some(label)) => format!("write {label}"),
            (Some(tool), Some(label)) => format!("{tool} {label}"),
            (Some(tool), None) => tool.to_owned(),
            (None, Some(label)) => label,
            (None, None) => "a call".to_owned(),
        }
    };
    let mut words = match item.kind {
        ContextPartKind::System => "the system prompt".to_owned(),
        ContextPartKind::Tools => "the tool definitions".to_owned(),
        ContextPartKind::Summary => "the summary".to_owned(),
        ContextPartKind::Prompts => "prompt".to_owned(),
        ContextPartKind::Preambles => "preamble".to_owned(),
        ContextPartKind::Notices => match &item.label {
            Some(job) => format!("notice of {}", one_line(job)),
            None => "notice".to_owned(),
        },
        ContextPartKind::Answers => "answer".to_owned(),
        ContextPartKind::ToolCalls => format!("call {}", call()),
        ContextPartKind::ToolOutputs => format!("output of {}", call()),
        ContextPartKind::Pruned => format!("pruned output of {}", call()),
        _ => "other".to_owned(),
    };
    if let Some(turn) = item.turn {
        let _ = write!(words, ", turn {turn}");
    }
    words
}

/// The count of the compactions and the newest one; `None` without one.
fn compactions(result: &ConversationContextResult, now: Timestamp) -> Option<String> {
    let summary = &result.compactions;
    if summary.count == 0 {
        return None;
    }
    let Some(newest) = &summary.newest else {
        return Some(summary.count.to_string());
    };
    let trigger = match newest.trigger {
        CompactionTrigger::Auto => "auto",
        CompactionTrigger::Manual => "efr compact",
        CompactionTrigger::Overflow => "the context was full",
        _ => "other",
    };
    let how = if newest.summarized { "with a summary" } else { "by pruning only" };
    Some(format!(
        "{}, the newest {} ({trigger}): {} -> {} {how}",
        summary.count,
        ago(newest.at, now),
        tokens(newest.tokens_before),
        tokens(newest.tokens_after),
    ))
}

/// The cache use: the share of the input that the cache served in the last turn and in
/// the whole conversation, and what the conversation wrote to the cache.
fn cache(result: &ConversationContextResult) -> String {
    let mut parts = Vec::new();
    if let Some(last) = result.cache.last_turn.filter(|last| last.input_tokens >= CACHE_SHOWN_FROM)
    {
        parts.push(format!("last turn {}%", read_percent(&last)));
    }
    let all = &result.cache.conversation;
    if all.input_tokens > 0 {
        let mut text = format!("conversation {}% read", read_percent(all));
        if all.cache_write_tokens > 0 {
            let _ = write!(text, ", {} written", tokens(all.cache_write_tokens));
            if all.cache_write_1h_tokens > 0 {
                let _ = write!(text, " ({} for 1h)", tokens(all.cache_write_1h_tokens));
            }
        }
        parts.push(text);
    }
    if parts.is_empty() {
        "no model call has reported its use yet".to_owned()
    } else {
        parts.join("; ")
    }
}

/// The part of the input that the cache served, in percent, rounded down.
fn read_percent(usage: &Usage) -> u128 {
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    u128::from(cached) * 100 / u128::from(usage.input_tokens).max(1)
}

#[cfg(test)]
mod tests;
