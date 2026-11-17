//! The wait for a person, as the model's question (`ask_user`) meets it: it waits
//! without end while a client that can answer follows and the user is there, waits the
//! away wait from the moment nobody is there, goes to the phones once with the call id,
//! and gives way to an answer, an interrupt and the approval timeout.

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{CallId, Event, QuestionId, QuestionReply, TurnId, UserAnswer};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::ask;
use crate::presence::PRESENCE_CHECK;
use crate::testing::{Harness, Script, Setup};

/// One `ask_user` call: which database, with two options.
pub(crate) fn ask_db() -> (&'static str, Value) {
    (
        ask::ASK_USER,
        json!({
            "question": "Which database?",
            "options": [
                { "label": "Postgres", "description": "Recommended." },
                { "label": "SQLite" },
            ],
        }),
    )
}

/// Starts `setup` with a model that makes `calls`, sends a prompt, and waits for the
/// first question of the model. A client that can answer follows when `followed`.
pub(crate) async fn asked(
    setup: Setup,
    calls: Vec<(&str, Value)>,
    followed: bool,
) -> (Harness, Arc<Script>, TurnId, QuestionId) {
    setup.presence.set_question_followed(followed);
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("set up the store").await;
    let question_id = h.wait_user_question().await;
    (h, script, sent.turn_id, question_id)
}

/// The end of the question `question_id` in the log.
pub(crate) async fn ended(h: &Harness, question_id: QuestionId) -> (UserAnswer, bool) {
    let events = h.events().await;
    let mut ends = events.iter().filter_map(|event| match event {
        Event::UserQuestionAnswered { question_id: id, answer, origin, .. }
            if *id == question_id =>
        {
            Some((answer.clone(), origin.is_some()))
        }
        _ => None,
    });
    let end = ends.next().expect("the question ended");
    assert!(ends.next().is_none(), "one end per question");
    end
}

/// The call of the first question of the model in the log.
async fn asking_call(h: &Harness) -> CallId {
    let events = h.events().await;
    events
        .iter()
        .find_map(|event| match event {
            Event::UserQuestionRequested { call_id, .. } => Some(*call_id),
            _ => None,
        })
        .expect("the question is in the log")
}

#[tokio::test]
async fn a_followed_question_waits_without_end_for_its_answer() {
    let (mut h, script, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], true).await;
    h.clock.advance(Duration::from_secs(3600));
    tokio::task::yield_now().await;
    assert!(!h.has_ended(turn_id), "no approval timeout is set, so it waits");
    assert!(h.presence.pushed().is_empty(), "a user who is there gets no push");

    h.reply(question_id, QuestionReply::Option { index: 0 }).await;
    h.wait_end(turn_id).await;

    assert_eq!(
        script.results(),
        vec![("The user chose option 1: \"Postgres\".".to_owned(), false)]
    );
    assert_eq!(
        ended(&h, question_id).await,
        (UserAnswer::Replied { reply: QuestionReply::Option { index: 0 } }, true)
    );
    assert!(h.toolbox.invoked().is_empty(), "the question never reaches the toolbox");
    assert!(h.toolbox.judged().is_empty(), "the engine never rules on the question");
}

#[tokio::test]
async fn a_question_that_nobody_can_answer_waits_the_away_wait_then_ends() {
    let (mut h, script, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], false).await;
    let call_id = asking_call(&h).await;
    h.clock.wait_for_sleeps(1).await;
    assert_eq!(h.presence.pushed(), vec![call_id], "the push names the call of the question");
    h.clock.advance(Duration::from_secs(299));
    assert!(!h.has_ended(turn_id), "it waits the whole away wait");
    h.clock.advance(Duration::from_secs(1));
    h.wait_end(turn_id).await;

    assert_eq!(script.results(), vec![(ask::unanswered(true), true)]);
    assert_eq!(ended(&h, question_id).await, (UserAnswer::Unanswered { away: true }, false));
    assert_eq!(h.presence.pushed(), vec![call_id], "one push per question");
}

#[tokio::test]
async fn an_away_user_gets_the_away_wait_even_with_a_client_that_follows() {
    let setup = Setup::new();
    setup.presence.set_away(true);
    let (mut h, script, turn_id, _) = asked(setup, vec![ask_db()], true).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(300));
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![(ask::unanswered(true), true)]);
}

#[tokio::test]
async fn an_answer_wins_during_the_away_wait() {
    let (mut h, script, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], false).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(200));
    h.reply(question_id, QuestionReply::Skipped).await;
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![(ask::SKIPPED.to_owned(), false)]);
}

#[tokio::test]
async fn a_client_that_comes_gives_the_normal_wait_again() {
    let (mut h, _, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], false).await;
    h.presence.wait_for_reads(1).await;
    h.presence.set_question_followed(true);
    h.clock.advance(PRESENCE_CHECK);
    h.presence.wait_for_reads(2).await;
    h.clock.advance(Duration::from_secs(600));
    tokio::task::yield_now().await;
    assert!(!h.has_ended(turn_id), "a followed question does not end");
    h.reply(question_id, QuestionReply::Text { text: "Postgres 17".to_owned() }).await;
    h.wait_end(turn_id).await;
}

#[tokio::test]
async fn a_client_that_leaves_starts_the_away_wait_from_then() {
    let (mut h, script, turn_id, _) = asked(Setup::new(), vec![ask_db()], true).await;
    h.presence.wait_for_reads(1).await;
    h.presence.set_question_followed(false);
    h.clock.advance(PRESENCE_CHECK);
    h.presence.wait_for_reads(2).await;
    h.clock.advance(Duration::from_secs(299));
    assert!(!h.has_ended(turn_id), "the away wait starts when efr sees nobody");
    h.clock.advance(Duration::from_secs(1));
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![(ask::unanswered(true), true)]);
}

#[tokio::test]
async fn the_approval_timeout_ends_a_followed_question() {
    let mut setup = Setup::new();
    setup.config.approval_timeout = Some(Duration::from_secs(60));
    let (mut h, script, turn_id, question_id) = asked(setup, vec![ask_db()], true).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(60));
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![(ask::unanswered(false), true)]);
    assert_eq!(ended(&h, question_id).await, (UserAnswer::Unanswered { away: false }, false));
}

#[tokio::test]
async fn an_interrupt_ends_the_question_and_the_turn() {
    let (mut h, _, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], true).await;
    let params = h.interrupt_params(Some(turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(params, efr_protocol::Origin::Shell).await.unwrap();
    let end = h.wait_end(turn_id).await;

    assert!(matches!(end, Event::TurnInterrupted { .. }), "{end:?}");
    assert_eq!(ended(&h, question_id).await, (UserAnswer::Interrupted, false));
    let refused = h.reply_from(question_id, QuestionReply::Skipped, efr_protocol::Origin::Shell);
    assert!(
        matches!(refused.await, Err(crate::ConversationError::QuestionNotPending { .. })),
        "an answer after the end finds no question"
    );
}
