//! The accounts of the model providers as the terminal names them (TM-Accounts, errata
//! 12): the kind of an account in words (`ChatGPT plan`), its name in a sentence (`b`,
//! `the ChatGPT plan`), the stop of a turn whose account is out with the offer of the
//! other accounts that serve its model, the limit rows of `efr status`, and the limit
//! of the status row of a running turn.
//!
//! An account is named as `,model` takes it: a named account by its name (`b` of
//! `openai-subscription.b`), the default account of a kind by its kind in words, such
//! as `the ChatGPT plan`. efrd decides which accounts the offer names; this file only
//! says them. A command row holds the command alone, so a triple click copies it.

use efr_protocol::{AccountLimits, AccountOut, AccountState, LimitKind, LimitWindow, OtherAccount};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::route::Pieces;
use super::status::account_of;
use super::{Tone, one_line};
use crate::error::login_command;

/// The kind of the provider id `id` in words, as `efr status` and the model sheet of
/// the phone name it: `Claude API`, `ChatGPT plan`, `OpenAI API`; another kind by its
/// id.
pub(crate) fn kind_words(id: &str) -> String {
    let kind = id.split_once('.').map_or(id, |(kind, _)| kind);
    match kind {
        "anthropic-api" => "Claude API".to_owned(),
        "openai-subscription" => "ChatGPT plan".to_owned(),
        "openai-api" => "OpenAI API".to_owned(),
        other => one_line(other),
    }
}

/// The account's own name, `b` of `openai-subscription.b`; `None` for the default
/// account of a kind, which has no name.
pub(crate) fn account_name(id: &str) -> Option<String> {
    id.split_once('.')
        .map(|(_, account)| account)
        .filter(|account| !account.is_empty())
        .map(one_line)
}

/// The default account of a kind in a sentence: `the ChatGPT plan`, `the OpenAI API
/// key`, `the Claude API key`.
fn default_name(id: &str) -> String {
    match id {
        "openai-subscription" => "the ChatGPT plan".to_owned(),
        "openai-api" => "the OpenAI API key".to_owned(),
        "anthropic-api" => "the Claude API key".to_owned(),
        other => format!("the account {}", one_line(other)),
    }
}

/// The account `id` inside a sentence: its name, such as `b`, or the default account
/// of its kind, such as `the ChatGPT plan` (TM-Accounts 2: "also runs on the ChatGPT
/// plan and b").
pub(crate) fn short_name(id: &str) -> String {
    account_name(id).unwrap_or_else(|| default_name(id))
}

/// The account `id` as the subject of a stop: `the account b`, or the default account
/// of its kind, such as `the ChatGPT plan`.
fn subject(id: &str) -> String {
    match account_name(id) {
        Some(name) => format!("the account {name}"),
        None => default_name(id),
    }
}

/// The command that switches the terminal to `model` on the account `id`: `,model
/// b/gpt-6.1-sol`.
pub(crate) fn model_command(id: &str, model: &str) -> String {
    format!(",model {}/{}", account_of(id).0, one_line(model))
}

/// When a limit resets, in the time zone `zone` at `now`: `14:20` on the same day,
/// `Mon 09:00` within a week, else `Nov 21 09:00`.
pub(crate) fn reset_time(at: Timestamp, now: Timestamp, zone: &TimeZone) -> String {
    let local = at.to_zoned(zone.clone());
    let today = now.to_zoned(zone.clone()).date();
    let days = (local.date() - today).get_days();
    let format = match days {
        0 => "%H:%M",
        1..7 => "%a %H:%M",
        _ => "%b %-d %H:%M",
    };
    jiff::fmt::strtime::format(format, &local).unwrap_or_else(|_| format!("{at:.0}"))
}

/// When a limit resets, inside a sentence: `at 14:20`, `on Mon at 09:00`.
fn reset_phrase(at: Timestamp, now: Timestamp, zone: &TimeZone) -> String {
    let time = reset_time(at, now, zone);
    match time.rsplit_once(' ') {
        Some((day, clock)) => format!("on {day} at {clock}"),
        None => format!("at {time}"),
    }
}

/// What the account did, after its name: `reached its usage limit`, `has no credit
/// left`. A stop of "too many requests" names no count of tries (errata 5).
fn limit_words(kind: LimitKind) -> &'static str {
    match kind {
        LimitKind::Usage => "reached its usage limit",
        LimitKind::Requests
        | LimitKind::Tokens
        | LimitKind::InputTokens
        | LimitKind::OutputTokens => "reached its rate limit",
        LimitKind::TooManyRequests => "got \"too many requests\", so efr stopped trying",
        LimitKind::Credit => "has no credit left",
        LimitKind::SpendLimit => "reached its spend limit",
        _ => "reached a limit",
    }
}

/// The reason under `✗ failed` when the account of the turn is out (TM-Accounts 1 to
/// 5): `the ChatGPT plan reached its usage limit. It resets at 14:20.` A known reset is
/// its own sentence; with none, no time.
pub(crate) fn stop_reason(out: &AccountOut, now: Timestamp, zone: &TimeZone) -> String {
    let mut reason = format!("{} {}.", subject(&out.provider), limit_words(out.reached.kind));
    if let Some(at) = out.reached.resets_at.filter(|_| out.reached.kind != LimitKind::Credit) {
        reason.push_str(&format!(" It resets {}.", reset_phrase(at, now, zone)));
    }
    reason
}

/// The reason of a failed turn in `efr history`, on the row of `✗ failed`
/// (TM-Accounts 7): `the ChatGPT plan reached its usage limit (resets 14:20)`. The
/// offer is not repeated: it is stale once a switch or a reset happened.
pub(crate) fn history_reason(out: &AccountOut, now: Timestamp, zone: &TimeZone) -> String {
    let mut reason = format!("{} {}", subject(&out.provider), limit_words(out.reached.kind));
    if let Some(at) = out.reached.resets_at.filter(|_| out.reached.kind != LimitKind::Credit) {
        reason.push_str(&format!(" (resets {})", reset_time(at, now, zone)));
    }
    reason
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The rows of the offer under the reason, 2 columns in (TM-Accounts 1, 2, 3, 5, 17):
/// one muted line naming the ready accounts, then a `,model` row in `code` for each
/// one; a full account whose reset comes first with its own line and row; a muted line
/// with the login command for each account that is signed out; and `no other account
/// runs <model>` when efrd names none. efr never switches by itself.
pub(crate) fn offer_rows(out: &AccountOut, now: Timestamp, zone: &TimeZone) -> Vec<Pieces> {
    let model = one_line(&out.model);
    let of = |state: AccountState| -> Vec<&OtherAccount> {
        out.others.iter().filter(|other| other.state == state).collect()
    };
    let (ready, full, signed_out) =
        (of(AccountState::Ready), of(AccountState::Full), of(AccountState::SignedOut));
    let muted = |text: String| vec![(text, Tone::Dim)];
    let command = |other: &OtherAccount| vec![(model_command(&other.provider, &model), Tone::Code)];
    let mut rows = Vec::new();
    if ready.is_empty() && full.is_empty() && signed_out.is_empty() {
        rows.push(muted(format!("no other account runs {model}")));
        return rows;
    }
    if !ready.is_empty() {
        let names: Vec<String> = ready.iter().map(|other| short_name(&other.provider)).collect();
        rows.push(muted(format!("{model} also runs on {}:", and_list(&names))));
        rows.extend(ready.iter().map(|other| command(other)));
    }
    for other in full {
        let when = other.resets_at.map(|at| format!(" {}", reset_phrase(at, now, zone)));
        let before = out
            .reached
            .resets_at
            .map(|_| format!(", before {}", short_name(&out.provider)))
            .unwrap_or_default();
        let name = short_name(&other.provider);
        let when = when.unwrap_or_default();
        rows.push(muted(format!("{model} runs again on {name}{when}{before}:")));
        rows.push(command(other));
    }
    for other in signed_out {
        let name = short_name(&other.provider);
        rows.push(muted(format!("{name} is signed out: {}", login_command(&other.provider))));
    }
    rows
}

/// The name of a usage window from its length: `5-hour`, `daily`, `weekly`, `monthly`,
/// else its length (`2-hour`, `3-day`), and `limit` when the provider tells none.
pub(crate) fn window_name(minutes: Option<u32>) -> String {
    match minutes {
        None => "limit".to_owned(),
        Some(1_440) => "daily".to_owned(),
        Some(10_080) => "weekly".to_owned(),
        Some(minutes) if (40_320..=44_640).contains(&minutes) => "monthly".to_owned(),
        Some(minutes) if minutes % 1_440 == 0 => format!("{}-day", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{}-hour", minutes / 60),
        Some(minutes) => format!("{minutes}-minute"),
    }
}

/// The name of a limit that an account reached and that is no window, for its row in
/// `efr status`: `requests`, `tokens`, `credit`.
fn reached_name(kind: LimitKind) -> &'static str {
    match kind {
        LimitKind::Usage => "usage",
        LimitKind::Requests => "requests",
        LimitKind::Tokens => "tokens",
        LimitKind::InputTokens => "input tokens",
        LimitKind::OutputTokens => "output tokens",
        LimitKind::Credit => "credit",
        LimitKind::SpendLimit => "spend",
        _ => "limit",
    }
}

/// The tone of how full a window is: `text` under 80 %, `warning` from 80 %, `error` at
/// 100 % (TM-Accounts 8: "only the number carries the level").
pub(crate) fn level_tone(percent: u8) -> Tone {
    match percent {
        100.. => Tone::Failure,
        80.. => Tone::Attention,
        _ => Tone::Text,
    }
}

/// One limit row of an account in `efr status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LimitRow {
    /// The window or the limit: `5-hour`, `requests`.
    pub(crate) name: String,
    /// How full the window is; `None` for a limit with no percent.
    pub(crate) percent: Option<u8>,
    /// The account reached this limit.
    pub(crate) reached: bool,
    /// When it resets, such as `14:20`.
    pub(crate) resets: Option<String>,
}

/// The limit rows of an account at `now` (TM-Accounts 8): one per window, in the order
/// of the provider, then a limit that it reached and that is no window, such as the
/// requests of an API key. A usage limit with a window at 100 % says `reached` on that
/// window's row.
pub(crate) fn limit_rows(limits: &AccountLimits, now: Timestamp, zone: &TimeZone) -> Vec<LimitRow> {
    let reset = |at: Option<Timestamp>| at.map(|at| reset_time(at, now, zone));
    let window = |window: &LimitWindow| LimitRow {
        name: window_name(window.minutes),
        percent: Some(window.used_percent.min(100)),
        reached: window.used_percent >= 100,
        resets: reset(window.resets_at),
    };
    let mut rows: Vec<LimitRow> = limits.windows.iter().map(window).collect();
    if let Some(reached) = &limits.reached {
        let shown = reached.kind == LimitKind::Usage && rows.iter().any(|row| row.reached);
        if !shown {
            rows.push(LimitRow {
                name: reached_name(reached.kind).to_owned(),
                percent: None,
                reached: true,
                resets: reset(reached.resets_at),
            });
        }
    }
    rows
}

/// How old limits must be before `efr status` says their age (TM-Accounts 8).
pub(crate) const STALE_AFTER: i64 = 5 * 60;

/// The limit that the status row of a running turn shows (TM-Accounts 10): the window
/// with the highest percent, from 90 %, such as `5-hour 92%`. Under 90 % nothing.
pub(crate) fn status_note(limits: &AccountLimits) -> Option<String> {
    let tightest = limits.windows.iter().max_by_key(|window| window.used_percent)?;
    (tightest.used_percent >= 90)
        .then(|| format!("{} {}%", window_name(tightest.minutes), tightest.used_percent.min(100)))
}

#[cfg(test)]
mod tests;
