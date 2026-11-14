//! The stop of a turn whose account is out, with the offer of the other accounts that
//! serve its model (TM-Accounts 1, 2, 5, 13, 14, 17; TM-Endings 25, 26; TM-Sequence
//! 15), in each depth and off a terminal; and the limit of the account in the status
//! row of a running turn (TM-Sequence 14).

use efr_protocol::{
    AccountLimits, AccountOut, AccountState, DraftPart, ErrorBody, ErrorCode, Event, LimitKind,
    LimitReached, LimitWindow, OtherAccount,
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
    };
    insta::assert_snapshot!(every_way(&[Sent(0, turn_started()), failed(38_000, out)]));
}

/// TM-Accounts 3 and 4: no other account runs the model.
#[test]
fn no_other_account_offers_nothing() {
    insta::assert_snapshot!(every_way(&[
        Sent(0, turn_started()),
        failed(12_000, plan_out(vec![]))
    ]));
}
