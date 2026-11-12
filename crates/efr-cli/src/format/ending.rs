//! The lines of the end of a turn: `done in 42s, ...`, the tokens of `efr history
//! --verbose`, why the model stopped, an interrupted turn, and a turn's settings. The
//! turn slice of the terminal look owns this file; `format.rs` re-exports its items, so
//! callers name them as `format::...`.

use std::fmt::Write as _;
use std::time::Duration;

use efr_protocol::{ContextUse, EffectiveSettings, StopReason, TurnStop, Usage};

use super::{context, one_line, tokens};

/// A turn's settings on one line, such as `mode auto, model gpt-5.4, effort high`: every
/// value with `all`, else only the ones that the prompt set. `None` when that leaves
/// none. An effort that is not sent shows as `default`, the backend's.
pub(crate) fn turn_settings(settings: &EffectiveSettings, all: bool) -> Option<String> {
    let EffectiveSettings { mode, model, effort, overridden, .. } = settings;
    let mut parts = Vec::new();
    if all || overridden.mode {
        parts.push(format!("mode {mode}"));
    }
    if all || overridden.model {
        parts.push(format!("model {}", one_line(model)));
    }
    if all || overridden.effort {
        parts.push(format!("effort {}", one_line(effort.as_deref().unwrap_or("default"))));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The smallest input of a turn, in tokens, for which the end-of-turn line shows the
/// share that the prompt cache served. Below the providers' minimums (512 tokens on
/// Claude, 1024 on OpenAI) no cache entry forms, so `cache 0%` there is only noise.
pub(crate) const CACHE_SHOWN_FROM: u64 = 2_048;

/// The end-of-turn line of a completed turn, such as `done in 42s, ctx 43% (89k/207k),
/// 1.1k out, cache 91%` when the turn says how full the context is, else `done in 42s,
/// 18.2k tokens in, 1.1k out, cache 91%`. The time and the tokens are left out when
/// they are not known, and the cache share when the input is below
/// [`CACHE_SHOWN_FROM`].
pub(crate) fn turn_done(
    took: Option<Duration>,
    usage: Option<&Usage>,
    context: Option<&ContextUse>,
) -> context::GaugedLine {
    let mut line = match took {
        Some(time) => format!("done in {}", super::took(time)),
        None => "done".to_owned(),
    };
    if let Some((gauge, rest)) = context.and_then(context::end_part) {
        line.push_str(", ");
        let mut after = rest;
        if let Some(usage) = usage {
            let _ = write!(after, ", {} out", tokens(usage.output_tokens));
            after.push_str(&cache_part(usage));
        }
        return context::GaugedLine::new(line, Some(gauge), after);
    }
    if let Some(usage) = usage {
        let _ = write!(
            line,
            ", {} tokens in, {} out",
            tokens(usage.input_tokens),
            tokens(usage.output_tokens)
        );
        line.push_str(&cache_part(usage));
    }
    line.into()
}

/// `, cache 91%`: the turn's cached input as a percent of its whole input, rounded
/// down, as Claude Code's hit ratio counts it, so cache writes count as misses. Empty
/// when the input is below [`CACHE_SHOWN_FROM`].
fn cache_part(usage: &Usage) -> String {
    if usage.input_tokens < CACHE_SHOWN_FROM {
        return String::new();
    }
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let percent = u128::from(cached) * 100 / u128::from(usage.input_tokens);
    format!(", cache {percent}%")
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
    line.push_str(&cache_part(usage));
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

/// Why the model of a completed turn stopped, when the turn records it: the model gave
/// no answer, or its answer stopped before its end, such as `the answer reached the
/// output limit (stop: max_tokens)`. Without this line, a turn that ends with no text
/// shows only `done`.
pub(crate) fn turn_stop(stop: &TurnStop) -> String {
    let reason = stop.reason.as_str();
    let what = match stop.reason {
        _ if stop.empty => "the model stopped with no answer",
        StopReason::MaxTokens => "the answer reached the output limit",
        StopReason::Refusal => "the model refused",
        StopReason::ContentFilter => "the provider stopped the answer for its content",
        StopReason::PauseTurn => "the provider paused the turn",
        _ => "the model stopped",
    };
    format!("{what} (stop: {reason})")
}

/// The end of an interrupted turn, such as `interrupted after 12s, ctx 43% (89k/207k)`;
/// `interrupted` alone when neither is known.
pub(crate) fn turn_interrupted(
    took: Option<Duration>,
    context: Option<&ContextUse>,
) -> context::GaugedLine {
    let line = match took {
        Some(time) => format!("interrupted after {}", super::took(time)),
        None => "interrupted".to_owned(),
    };
    match context.and_then(context::end_part) {
        Some((gauge, rest)) => context::GaugedLine::new(format!("{line}, "), Some(gauge), rest),
        None => line.into(),
    }
}
