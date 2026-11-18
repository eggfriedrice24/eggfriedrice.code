//! The stop of a turn whose account is out, with the offer of the other accounts that
//! serve its model (TM-Accounts 1, 2, 5, 13, 14, 17; TM-Endings 25, 26; TM-Sequence
//! 15), in each depth and off a terminal; and the limit of the account in the status
//! row of a running turn (TM-Sequence 14).

use efr_protocol::{
    AccountLimits, AccountOut, AccountState, DraftPart, EffectiveSettings, ErrorBody, ErrorCode,
    Event, LimitKind, LimitReached, LimitWindow, Mode, OtherAccount, OverriddenSettings,
};
use jiff::SignedDuration;

use super::super::at;
use super::{Act, every_way, message, turn_started};
use crate::testing::turn;

use Act::{Drafted, Event as Sent, Look as Shot};

/// TM-Sequence 14: the `limits` draft of the turn's account puts the fullest window in
/// the status row from 90 %; a later draft under 90 % takes it away.
#[test]
fn the_limits_draft_puts_the_fullest_window_in_the_status_row() {
    let limits = |percent: u8| DraftPart::Limits {
        provider: "openai-subscription".to_owned(),
        limits: AccountLimits {
            observed_at: at(0),
            windows: vec![
                LimitWindow { minutes: Some(300), used_percent: percent, resets_at: None },
                LimitWindow { minutes: Some(10_080), used_percent: 41, resets_at: None },
            ],
            reached: None,
        },
    };
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Drafted(limits(96)),
        Shot("close to the limit"),
        Drafted(limits(30)),
        Shot("after a reset"),
    ]));
}

/// The start of a turn on gpt-6.1-sol medium of the default ChatGPT plan; `many` when
/// another account lists the model too.
fn plan_turn_started(many: bool) -> Event {
    let Event::TurnStarted { turn_id, cwd, scope, .. } = turn_started() else {
        unreachable!("turn_started() is a turn_started event");
    };
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-6.1-sol".to_owned(),
        effort: Some("medium".to_owned()),
        default_effort: None,
        provider: Some("openai-subscription".to_owned()),
        many_accounts: many,
        overridden: OverriddenSettings::default(),
        fallback: None,
    };
    Event::TurnStarted {
        turn_id,
        cwd,
        scope,
        settings: Some(settings),
        unread_attachments: Vec::new(),
    }
}

/// TM-Accounts 10 and 18 (D1): the status row names the default account of a kind only
/// while another account lists the model too, as `,model` takes it; else the model is
/// bare (TM-Legend 53).
#[test]
fn the_status_row_names_the_default_account_while_two_accounts_list_the_model() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, plan_turn_started(true)),
        Shot("b lists the model too"),
        Sent(500, plan_turn_started(false)),
        Shot("one account lists the model"),
    ]));
}

/// The turn fails `millis` after the prompt because its account is `out`.
fn failed(millis: i64, out: AccountOut) -> Act {
    Sent(
        millis,
        Event::TurnFailed {
            turn_id: turn(),
            error: ErrorBody::new(
                ErrorCode::Busy,
                "the account reached the usage limit of its plan",
            ),
            usage: None,
            context: None,
            changes: None,
            undo: None,
            account_out: Some(out),
        },
    )
}

fn other(provider: &str, state: AccountState) -> OtherAccount {
    OtherAccount { provider: provider.to_owned(), label: None, state, resets_at: None }
}

/// The ChatGPT plan at its usage limit, which resets at 14:20 (the tests start at
/// 12:00 UTC).
fn plan_out(others: Vec<OtherAccount>) -> AccountOut {
    AccountOut {
        provider: "openai-subscription".to_owned(),
        label: None,
        model: "gpt-6.1-sol".to_owned(),
        reached: LimitReached {
            kind: LimitKind::Usage,
            resets_at: Some(at(0) + SignedDuration::from_mins(140)),
        },
        others,
        others_full: None,
    }
}

/// TM-Accounts 1 (with errata 2), 13 and 14: one other account, its command alone on
/// its row, in `code` and with no backticks in NO_COLOR.
#[test]
fn an_account_at_its_limit_offers_the_other_account_of_the_model() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        Sent(500, message(0, "The store docs build. I add a page on the import next.")),
        failed(130_000, plan_out(vec![other("openai-subscription.b", AccountState::Ready)])),
    ]));
}

/// TM-Accounts 5 (TM-Endings 26) with errata 5: the other account is signed out, so
/// nothing is offered; one muted line names its login.
#[test]
fn a_signed_out_account_is_never_offered() {
    let out = AccountOut {
        provider: "anthropic-api.work".to_owned(),
        label: None,
        model: "claude-opus-5-5".to_owned(),
        reached: LimitReached { kind: LimitKind::TooManyRequests, resets_at: None },
        others: vec![other("anthropic-api.personal", AccountState::SignedOut)],
        others_full: None,
    };
    insta::assert_snapshot!(every_way(&[Sent(0, turn_started()), failed(38_000, out)]));
}

/// D3: b runs the model too, but it is full until after the plan: no other account has
/// room for the model before b's reset.
#[test]
fn accounts_that_are_full_for_longer_have_no_room_before_their_reset() {
    let mut out = plan_out(vec![]);
    out.others_full =
        Some(efr_protocol::OthersFull { resets_at: Some(at(0) + SignedDuration::from_mins(200)) });
    insta::assert_snapshot!(every_way(&[Sent(0, turn_started()), failed(12_000, out)]));
}

/// TM-Accounts 3 and 4: no other account runs the model.
#[test]
fn no_other_account_offers_nothing() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        failed(12_000, plan_out(vec![]))
    ]));
}
