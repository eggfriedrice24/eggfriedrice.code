//! The limits of an account, as OpenAI's servers tell them in their answers.
//!
//! A ChatGPT plan tells how full its usage windows are: in the headers of each answer
//! of the subscription backend (`x-codex-primary-used-percent`, `-window-minutes`,
//! `-reset-at`, and the same for `secondary`), and on the WebSocket in a
//! `codex.rate_limits` message, with the same values as JSON (Codex
//! `codex-rs/codex-api/src/rate_limits.rs`). efr reads only the default family
//! `codex`. A family of one model, such as `x-codex-other-primary-used-percent`, has no
//! row of its own in efr yet, so efr ignores it.
//!
//! An API key tells no windows, only buckets per minute (`x-ratelimit-remaining-*`,
//! `x-ratelimit-reset-*`). efr reads them only from a 429 that outlived the retries, to
//! name the bucket that ran out and its reset ([`exhausted_bucket`]). A key that serves
//! shows no limit: the boards show the rate limit of a key only when it was hit.

use efr_http::HeaderMap;
use efr_provider::{LimitKind, LimitReached, LimitWindow};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

/// The prefix of the headers of the default family of a ChatGPT plan's limits.
const PLAN_PREFIX: &str = "x-codex";

/// The two windows of a plan, in the order of the provider: the short one, then the
/// long one.
const PLAN_WINDOWS: [&str; 2] = ["primary", "secondary"];

/// The id of the default family of a plan's limits in a `codex.rate_limits` message.
const DEFAULT_FAMILY: &str = "codex";

/// The buckets of an API key: the kind, the header of what remains, and the header of
/// when the bucket is full again.
const BUCKETS: [(LimitKind, &str, &str); 3] = [
    (LimitKind::Requests, "x-ratelimit-remaining-requests", "x-ratelimit-reset-requests"),
    (LimitKind::Tokens, "x-ratelimit-remaining-tokens", "x-ratelimit-reset-tokens"),
    (LimitKind::Tokens, "x-ratelimit-remaining-project-tokens", "x-ratelimit-reset-project-tokens"),
];

/// The usage windows of a ChatGPT plan in the headers of an answer; empty when the
/// headers tell none, as for an API key.
pub(crate) fn plan_windows(headers: &HeaderMap) -> Vec<LimitWindow> {
    PLAN_WINDOWS
        .iter()
        .filter_map(|window| {
            let header = |part: &str| {
                headers
                    .get(format!("{PLAN_PREFIX}-{window}-{part}"))
                    .and_then(|value| value.to_str().ok())
                    .map(str::trim)
            };
            let percent = header("used-percent")?.parse::<f64>().ok()?;
            let minutes = header("window-minutes").and_then(|text| text.parse::<i64>().ok());
            let reset_at = header("reset-at").and_then(|text| text.parse::<i64>().ok());
            window_of(percent, minutes, reset_at)
        })
        .collect()
}

/// The usage windows of a `codex.rate_limits` message of the WebSocket, of the default
/// family only; `None` for any other message.
pub(crate) fn plan_windows_message(event: &Value) -> Option<Vec<LimitWindow>> {
    if event.get("type").and_then(Value::as_str) != Some("codex.rate_limits") {
        return None;
    }
    let family = event
        .get("metered_limit_name")
        .or_else(|| event.get("limit_name"))
        .and_then(Value::as_str)
        .map(|name| name.trim().to_ascii_lowercase().replace('-', "_"));
    if family.is_some_and(|family| family != DEFAULT_FAMILY) {
        return Some(Vec::new());
    }
    let limits = event.get("rate_limits");
    let windows = PLAN_WINDOWS
        .iter()
        .filter_map(|name| {
            let window = limits?.get(name)?;
            let percent = window.get("used_percent").and_then(Value::as_f64)?;
            let minutes = window.get("window_minutes").and_then(Value::as_i64);
            let reset_at = window.get("reset_at").and_then(Value::as_i64);
            window_of(percent, minutes, reset_at)
        })
        .collect();
    Some(windows)
}

/// One window from the values of the provider. A window that is unused, with no length
/// and no reset, tells nothing, so it is absent, as Codex reads it.
fn window_of(percent: f64, minutes: Option<i64>, reset_at: Option<i64>) -> Option<LimitWindow> {
    if !percent.is_finite() {
        return None;
    }
    let minutes = minutes.filter(|minutes| *minutes > 0).and_then(|m| u32::try_from(m).ok());
    let resets_at = reset_at.and_then(|at| Timestamp::from_second(at).ok());
    if percent == 0.0 && minutes.is_none() && resets_at.is_none() {
        return None;
    }
    // NOTE: rounded down, so a window shows 100 only when the provider says it is full.
    // The clamp keeps the cast inside the range of u8.
    let used_percent = percent.clamp(0.0, 100.0).floor() as u8;
    Some(LimitWindow { minutes, used_percent, resets_at })
}

/// The bucket of an API key that ran out, from the headers of a 429: the one with
/// nothing left whose reset comes last, because the key serves again only when each
/// one is full again. Its reset is `now` plus the wait of its header, else the end of
/// `retry_after`. `None` when no bucket in the headers is empty: the server then only
/// said "too many requests".
pub(crate) fn exhausted_bucket(
    headers: &HeaderMap,
    now: Timestamp,
    retry_after: Option<std::time::Duration>,
) -> Option<LimitReached> {
    let fallback = retry_after
        .and_then(|wait| SignedDuration::try_from(wait).ok())
        .and_then(|wait| now.checked_add(wait).ok());
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    BUCKETS
        .iter()
        .filter(|(_, remaining, _)| text(remaining).and_then(|n| n.parse::<u64>().ok()) == Some(0))
        .map(|(kind, _, reset)| {
            let resets_at = text(reset)
                .and_then(reset_wait)
                .and_then(|wait| now.checked_add(wait).ok())
                .or(fallback);
            LimitReached { kind: *kind, resets_at }
        })
        .max_by_key(|reached| reached.resets_at)
}

/// The wait of an `x-ratelimit-reset-*` header, such as `1s`, `6m0s`, `20ms` or
/// `1h2m3.5s`: numbers, each with its unit `h`, `m`, `s` or `ms`. The docs give only
/// samples, so anything else is `None`.
fn reset_wait(text: &str) -> Option<SignedDuration> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0_f64;
    while !rest.is_empty() {
        let number_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        let number: f64 = rest.get(..number_end)?.parse().ok()?;
        rest = rest.get(number_end..)?;
        let unit_end = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        let seconds = match rest.get(..unit_end)? {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 0.001,
            _ => return None,
        };
        total += number * seconds;
        rest = rest.get(unit_end..)?;
    }
    SignedDuration::try_from_secs_f64(total).ok()
}

#[cfg(test)]
mod tests;
