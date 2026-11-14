use efr_protocol::{
    AccountLimits, AccountOut, AccountState, LimitKind, LimitReached, LimitWindow, OtherAccount,
};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{
    LimitRow, history_reason, kind_words, limit_rows, offer_rows, reset_time, short_name,
    status_note, stop_reason, window_name,
};
use crate::format::Tone;
use crate::format::ending::plain;
use crate::testing::now;

/// `now` (a Sunday, 12:00 UTC) and `minutes` later.
fn at(minutes: i64) -> Timestamp {
    now() + SignedDuration::from_mins(minutes)
}

/// 14:20 of the same day.
fn at_1420() -> Timestamp {
    at(140)
}

fn other(provider: &str, state: AccountState, resets_at: Option<Timestamp>) -> OtherAccount {
    OtherAccount { provider: provider.to_owned(), label: None, state, resets_at }
}

fn out(provider: &str, model: &str, kind: LimitKind, resets_at: Option<Timestamp>) -> AccountOut {
    AccountOut {
        provider: provider.to_owned(),
        label: None,
        model: model.to_owned(),
        reached: LimitReached { kind, resets_at },
        others: Vec::new(),
    }
}

/// The stop and its offer as rows of plain text, with the tone of each row.
fn shown(out: &AccountOut) -> Vec<String> {
    let zone = TimeZone::UTC;
    let mut rows = vec![stop_reason(out, now(), &zone)];
    for row in offer_rows(out, now(), &zone) {
        let tones: Vec<&str> = row
            .iter()
            .map(|(_, tone)| match tone {
                Tone::Code => "code",
                Tone::Dim => "muted",
                _ => "other",
            })
            .collect();
        rows.push(format!("{} [{}]", plain(&row), tones.join(",")));
    }
    rows
}

#[test]
fn the_kinds_and_the_names_are_those_of_the_design() {
    assert_eq!(kind_words("anthropic-api.work"), "Claude API");
    assert_eq!(kind_words("openai-subscription"), "ChatGPT plan");
    assert_eq!(kind_words("openai-api"), "OpenAI API");
    assert_eq!(kind_words("other-kind.x"), "other-kind");
    assert_eq!(short_name("openai-subscription.b"), "b");
    assert_eq!(short_name("openai-subscription"), "the ChatGPT plan");
    assert_eq!(short_name("openai-api"), "the OpenAI API key");
}

/// TM-Accounts 1 with errata 2: the default plan is out, b has room.
#[test]
fn one_other_account_is_offered_with_its_command_alone_on_its_row() {
    let mut out = out("openai-subscription", "gpt-6.1-sol", LimitKind::Usage, Some(at_1420()));
    out.others = vec![other("openai-subscription.b", AccountState::Ready, None)];
    assert_eq!(
        shown(&out),
        [
            "the ChatGPT plan reached its usage limit. It resets at 14:20.",
            "gpt-6.1-sol also runs on b: [muted]",
            ",model b/gpt-6.1-sol [code]",
        ]
    );
}

/// TM-Accounts 2: no reset, two other accounts, the default plan by its `,model` name.
#[test]
fn two_other_accounts_each_get_a_row_and_no_reset_gives_no_time() {
    let mut out = out("openai-api", "gpt-6.1", LimitKind::Credit, None);
    out.others = vec![
        other("openai-subscription", AccountState::Ready, None),
        other("openai-subscription.b", AccountState::Ready, None),
    ];
    assert_eq!(
        shown(&out),
        [
            "the OpenAI API key has no credit left.",
            "gpt-6.1 also runs on the ChatGPT plan and b: [muted]",
            ",model openai/gpt-6.1 [code]",
            ",model b/gpt-6.1 [code]",
        ]
    );
}

/// TM-Accounts 3 and 4: no other account says so and offers nothing.
#[test]
fn no_other_account_says_so() {
    let limited = out("openai-api", "gpt-5.5", LimitKind::Requests, Some(at(123)));
    assert_eq!(
        shown(&limited),
        [
            "the OpenAI API key reached its rate limit. It resets at 14:03.",
            "no other account runs gpt-5.5 [muted]",
        ]
    );
    let empty = out("openai-api", "gpt-5.5", LimitKind::Credit, None);
    assert_eq!(
        shown(&empty),
        ["the OpenAI API key has no credit left.", "no other account runs gpt-5.5 [muted]"]
    );
}

/// TM-Accounts 5 with errata 5: no count of tries; a signed-out account is never
/// offered, it gets its login command in one muted line.
#[test]
fn a_signed_out_account_gets_its_login_command_and_no_offer() {
    let mut out = out("anthropic-api.work", "claude-opus-5-5", LimitKind::TooManyRequests, None);
    out.others = vec![other("anthropic-api.personal", AccountState::SignedOut, None)];
    assert_eq!(
        shown(&out),
        [
            "work got \"too many requests\", so efr stopped trying.",
            "personal is signed out: efr login anthropic --account personal [muted]",
        ]
    );
}

/// TM-Accounts 17: a full account whose reset comes first.
#[test]
fn a_full_account_whose_reset_comes_first_says_when_it_runs_again() {
    let mut out = out("openai-subscription.b", "gpt-6.1-sol", LimitKind::Usage, Some(at(245)));
    out.others = vec![other("openai-subscription", AccountState::Full, Some(at_1420()))];
    assert_eq!(
        shown(&out),
        [
            "b reached its usage limit. It resets at 16:05.",
            "gpt-6.1-sol runs again on the ChatGPT plan at 14:20, before b: [muted]",
            ",model openai/gpt-6.1-sol [code]",
        ]
    );
}

#[test]
fn a_reset_on_another_day_names_the_day() {
    let zone = TimeZone::UTC;
    // Monday 09:00 is 21 hours after Sunday 12:00.
    assert_eq!(reset_time(at(21 * 60), now(), &zone), "Mon 09:00");
    assert_eq!(reset_time(at(10 * 24 * 60), now(), &zone), "Oct 14 12:00");
    let out = out("openai-subscription", "gpt-6.1-sol", LimitKind::Usage, Some(at(21 * 60)));
    assert_eq!(
        stop_reason(&out, now(), &zone),
        "the ChatGPT plan reached its usage limit. It resets on Mon at 09:00."
    );
}

/// TM-Accounts 7: the reason stays in `efr history`, on one row.
#[test]
fn the_history_reason_keeps_the_reset_in_brackets() {
    let zone = TimeZone::UTC;
    let out = out("openai-subscription", "gpt-6.1-sol", LimitKind::Usage, Some(at_1420()));
    assert_eq!(
        history_reason(&out, now(), &zone),
        "the ChatGPT plan reached its usage limit (resets 14:20)"
    );
    // NOTE: errata 12: a named account by its name alone, as the offer and `efr status`.
    let named = AccountOut { provider: "openai-subscription.b".to_owned(), ..out };
    assert_eq!(history_reason(&named, now(), &zone), "b reached its usage limit (resets 14:20)");
}

#[test]
fn windows_are_named_by_their_length() {
    assert_eq!(window_name(Some(300)), "5-hour");
    assert_eq!(window_name(Some(10_080)), "weekly");
    assert_eq!(window_name(Some(1_440)), "daily");
    assert_eq!(window_name(Some(43_200)), "monthly");
    assert_eq!(window_name(Some(4_320)), "3-day");
    assert_eq!(window_name(Some(45)), "45-minute");
    assert_eq!(window_name(None), "limit");
}

fn window(minutes: u32, used_percent: u8, resets_at: Option<Timestamp>) -> LimitWindow {
    LimitWindow { minutes: Some(minutes), used_percent, resets_at }
}

/// TM-Accounts 8: a window at 100 % says `reached`; the requests of a key show only
/// when they were hit.
#[test]
fn limit_rows_hold_the_windows_then_a_limit_that_is_no_window() {
    let zone = TimeZone::UTC;
    let plan = AccountLimits {
        observed_at: at(-12),
        windows: vec![window(300, 100, Some(at_1420())), window(10_080, 63, Some(at(21 * 60)))],
        reached: Some(LimitReached { kind: LimitKind::Usage, resets_at: Some(at_1420()) }),
    };
    assert_eq!(
        limit_rows(&plan, now(), &zone),
        [
            LimitRow {
                name: "5-hour".to_owned(),
                percent: Some(100),
                reached: true,
                resets: Some("14:20".to_owned()),
            },
            LimitRow {
                name: "weekly".to_owned(),
                percent: Some(63),
                reached: false,
                resets: Some("Mon 09:00".to_owned()),
            },
        ]
    );
    let key = AccountLimits {
        observed_at: now(),
        windows: Vec::new(),
        reached: Some(LimitReached { kind: LimitKind::Requests, resets_at: Some(at(123)) }),
    };
    assert_eq!(
        limit_rows(&key, now(), &zone),
        [LimitRow {
            name: "requests".to_owned(),
            percent: None,
            reached: true,
            resets: Some("14:03".to_owned()),
        }]
    );
}

/// TM-Accounts 10: the tightest window, from 90 %.
#[test]
fn the_status_row_shows_the_tightest_window_from_90_percent() {
    let limits = |windows| AccountLimits { observed_at: now(), windows, reached: None };
    assert_eq!(status_note(&limits(vec![window(300, 89, None)])), None);
    assert_eq!(
        status_note(&limits(vec![window(300, 92, None), window(10_080, 41, None)])),
        Some("5-hour 92%".to_owned())
    );
    assert_eq!(
        status_note(&limits(vec![window(300, 30, None), window(10_080, 96, None)])),
        Some("weekly 96%".to_owned())
    );
    assert_eq!(status_note(&limits(Vec::new())), None);
}
