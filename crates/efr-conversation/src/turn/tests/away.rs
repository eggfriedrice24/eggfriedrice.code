//! A question while the user is away: it waits `presence.away_wait_secs`, then the call
//! does not run and the model goes on without it.

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{ApprovalDecision, Event, JudgeKind, Verdict};
use pretty_assertions::assert_eq;

use super::review::{judgements, reviewed, shell};
use crate::exit::AWAY_EXPIRED;
use crate::testing::{Harness, Script, Setup};

/// Starts a turn of `setup`, whose model makes one unnamed push, a question for the user,
/// and waits for the question. The user is away when `away`.
async fn asked(
    setup: Setup,
    away: bool,
) -> (Harness, Arc<Script>, efr_protocol::TurnId, efr_protocol::CallId) {
    setup.presence.set_away(away);
    let script = Script::new(vec![shell("git push origin main")]);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("fix the typo").await;
    let call_id = h.wait_approval().await;
    (h, script, sent.turn_id, call_id)
}

#[tokio::test]
async fn away_question_expires_and_call_not_run() {
    let (mut h, _, turn_id, call_id) = asked(reviewed(None), true).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(299));
    assert!(!h.has_ended(turn_id), "it waits the whole away wait");
    h.clock.advance(Duration::from_secs(1));
    let end = h.wait_end(turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "the turn goes on: {end:?}");
    assert!(h.toolbox.invoked().is_empty());
    let events = h.events().await;
    assert!(events.contains(&Event::ApprovalExpired { turn_id, call_id, away: true }));
    assert_eq!(judgements(&h).await, vec![(JudgeKind::PresenceExpired, Verdict::Deny)]);
}

#[tokio::test]
async fn away_expiry_message_tells_model_to_continue() {
    let mut setup = reviewed(None);
    setup.config.away_wait = Duration::from_secs(60);
    let (mut h, script, turn_id, _) = asked(setup, true).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(turn_id).await;

    assert_eq!(script.results(), vec![(AWAY_EXPIRED.to_owned(), true)]);
    assert_eq!(
        AWAY_EXPIRED,
        "Nobody could approve this now; it did not run. Continue without it, and name it in \
         your final answer."
    );
}

#[tokio::test]
async fn away_push_sent_once_per_question() {
    let (mut h, _, turn_id, call_id) = asked(reviewed(None), true).await;
    // The push goes out after the question is recorded, before the wait starts.
    h.clock.wait_for_sleeps(1).await;
    assert_eq!(h.presence.pushed(), vec![call_id]);
    h.answer(call_id, ApprovalDecision::Allow).await;
    h.wait_end(turn_id).await;

    assert_eq!(h.presence.pushed(), vec![call_id], "one push, also after the answer");
    assert_eq!(h.toolbox.invoked().len(), 1, "an answer during the away wait wins");
    let events = h.events().await;
    assert!(!events.iter().any(|e| matches!(e, Event::ApprovalExpired { .. })));
}

#[tokio::test]
async fn a_present_user_gets_no_push_and_no_away_wait() {
    let (mut h, _, turn_id, call_id) = asked(reviewed(None), false).await;
    assert!(h.presence.pushed().is_empty());
    // No approval timeout is set, so the question waits for the answer.
    tokio::task::yield_now().await;
    assert!(!h.has_ended(turn_id));
    h.answer(call_id, ApprovalDecision::Deny).await;
    h.wait_end(turn_id).await;
    assert_eq!(judgements(&h).await, vec![(JudgeKind::User, Verdict::Deny)]);
}
