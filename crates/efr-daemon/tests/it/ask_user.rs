//! The model's question over a `TestDaemon` (`ask_user`): `question.respond` answers it
//! with an option, a text or a skip, from the terminal or a phone; a retry answers from
//! its receipt; a bad answer is `invalid`; with no client that can answer, the question
//! waits the away wait; an interrupt and a restart end it; `conversation.ask_user =
//! false` offers no tool.

use std::sync::Arc;
use std::time::Duration;

use efr_conversation::PRESENCE_CHECK;
use efr_protocol::{
    ConversationSubscribeItem, ErrorCode, Event, EventEnvelope, Method, Origin, PromptSendResult,
    QuestionId, QuestionReply, QuestionRespondResult, TurnInterrupt, TurnInterruptResult,
    UserAnswer,
};
use efr_provider::Provider;
use efr_test_daemon::{
    Client, ClientError, ItemStream, TTY, TestDaemon, TestDaemonBuilder, answer_question,
    command_id, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, kinds};
use crate::support::outputs;

/// The question of the model: which database, with two options.
fn which_database() -> Value {
    json!({
        "question": "Which database?",
        "options": [{ "label": "Postgres", "description": "Recommended." }, { "label": "SQLite" }],
    })
}

/// A daemon of `builder` whose model asks [`which_database`], then answers.
async fn asking(builder: TestDaemonBuilder) -> TestDaemon {
    let model = ScriptedModel::reviewed(vec![("ask_user", which_database())], REVIEWER_ALLOWS);
    builder.custom_provider(model as Arc<dyn Provider>).start().await.unwrap()
}

/// Sends the prompt from [`TTY`], follows its conversation with `follow` (a client
/// that can answer when `answers`), and waits for the model's question.
async fn asked(
    daemon: &TestDaemon,
    client: &Client,
    answers: bool,
) -> (PromptSendResult, ItemStream<ConversationSubscribeItem>, QuestionId) {
    let sent: PromptSendResult =
        client.call(daemon.prompt(1, "set up the store", TTY)).await.unwrap();
    let mut follow = if answers {
        daemon.follow_answering(client, sent.conversation_id).await.unwrap()
    } else {
        daemon.follow(client, sent.conversation_id).await.unwrap()
    };
    let events =
        events_until(&mut follow, |event| matches!(event, Event::UserQuestionRequested { .. }))
            .await
            .unwrap();
    let question_id = events
        .iter()
        .find_map(|envelope| match envelope.event {
            Event::UserQuestionRequested { question_id, .. } => Some(question_id),
            _ => None,
        })
        .unwrap();
    (sent, follow, question_id)
}

/// The events of `follow` up to the end of a turn.
async fn until_end(follow: &mut ItemStream<ConversationSubscribeItem>) -> Vec<EventEnvelope> {
    events_until(follow, |event| {
        matches!(
            event,
            Event::TurnCompleted { .. } | Event::TurnFailed { .. } | Event::TurnInterrupted { .. }
        )
    })
    .await
    .unwrap()
}

/// How the model's question ended in `events`, and who answered.
fn end(events: &[EventEnvelope]) -> Option<(UserAnswer, Option<Origin>)> {
    events.iter().find_map(|envelope| match &envelope.event {
        Event::UserQuestionAnswered { answer, origin, .. } => Some((answer.clone(), *origin)),
        _ => None,
    })
}

/// Moves the clock past the away wait once the question sleeps on it.
/// The away wait of the question is the first sleep of its length after the first look
/// of the question ([`PRESENCE_CHECK`]) that came after the first `after` sleeps of
/// the clock. The job guard of efrd sleeps as long from its start, and a move before
/// the question sleeps would leave the question waiting.
async fn pass_the_away_wait(daemon: &TestDaemon, after: usize) {
    let clock = daemon.clock();
    let away_wait = Duration::from_secs(300);
    Wait::new("the away wait of the question")
        .until_some(|| {
            let sleeps = clock.requested_sleeps();
            let look = sleeps.iter().skip(after).position(|&sleep| sleep == PRESENCE_CHECK)?;
            sleeps[after + look..].contains(&away_wait).then_some(())
        })
        .await
        .unwrap();
    clock.advance(away_wait);
}

/// The error code of a refused call.
fn code<T: std::fmt::Debug>(refused: Result<T, ClientError>) -> ErrorCode {
    match refused {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("a refusal: {other:?}"),
    }
}

#[tokio::test]
async fn hello_announces_the_model_s_question() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().capabilities.user_questions, Some(true));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn an_option_a_text_and_a_skip_reach_the_model() {
    let cases = [
        (QuestionReply::Option { index: 1 }, "The user chose option 2: \"SQLite\".", false),
        (
            QuestionReply::Text { text: "Postgres 17".to_owned() },
            "The user answered in their own words: \"Postgres 17\"",
            false,
        ),
        (QuestionReply::Skipped, "The user skipped the question.", false),
    ];
    for (reply, wanted, is_error) in cases {
        let daemon = asking(TestDaemon::builder()).await;
        let client = daemon.client_for_tty(TTY).await.unwrap();
        let (sent, mut follow, question_id) = asked(&daemon, &client, true).await;
        let answer = answer_question(2, sent.conversation_id, question_id, reply.clone());
        let answered: QuestionRespondResult = client.call(answer).await.unwrap();
        let events = until_end(&mut follow).await;

        assert!(answered.seq.get() > 0);
        let read = outputs(&events);
        assert_eq!(read.len(), 1, "{read:?}");
        assert!(read[0].0.starts_with(wanted), "{reply:?}: {read:?}");
        assert_eq!(read[0].1, is_error);
        assert_eq!(end(&events), Some((UserAnswer::Replied { reply }, Some(Origin::Shell))));
        drop((follow, client));
        daemon.stop().await.unwrap();
    }
}

#[tokio::test]
async fn a_retry_answers_from_its_receipt_and_a_bad_answer_is_invalid() {
    let daemon = asking(TestDaemon::builder()).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (sent, mut follow, question_id) = asked(&daemon, &client, true).await;
    let conversation_id = sent.conversation_id;

    let outside =
        answer_question(2, conversation_id, question_id, QuestionReply::Option { index: 2 });
    assert_eq!(code(client.call::<QuestionRespondResult>(outside).await), ErrorCode::Invalid);
    let empty = answer_question(
        3,
        conversation_id,
        question_id,
        QuestionReply::Text { text: String::new() },
    );
    assert_eq!(code(client.call::<QuestionRespondResult>(empty).await), ErrorCode::Invalid);
    let nothing = QuestionId::from_uuid(uuid::Uuid::from_u128(1));
    let unknown = answer_question(4, conversation_id, nothing, QuestionReply::Skipped);
    assert_eq!(code(client.call::<QuestionRespondResult>(unknown).await), ErrorCode::NotFound);

    let answer = answer_question(5, conversation_id, question_id, QuestionReply::Skipped);
    let first: QuestionRespondResult = client.call(answer.clone()).await.unwrap();
    let again: QuestionRespondResult = client.call(answer).await.unwrap();
    assert_eq!(again, first, "a retried command answers from its receipt");
    until_end(&mut follow).await;
    let late = answer_question(6, conversation_id, question_id, QuestionReply::Skipped);
    assert_eq!(code(client.call::<QuestionRespondResult>(late).await), ErrorCode::NotFound);
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_phone_may_answer_the_model_s_question() {
    let daemon = asking(TestDaemon::builder()).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let (sent, _follow, question_id) = asked(&daemon, &client, false).await;
    let mut watch = daemon.follow_answering(&phone, sent.conversation_id).await.unwrap();

    let answer =
        answer_question(2, sent.conversation_id, question_id, QuestionReply::Option { index: 0 });
    let _: QuestionRespondResult = phone.call(answer).await.unwrap();
    let events = until_end(&mut watch).await;

    let reply = QuestionReply::Option { index: 0 };
    assert_eq!(end(&events), Some((UserAnswer::Replied { reply }, Some(Origin::Phone))));
    assert_eq!(
        outputs(&events),
        vec![("The user chose option 1: \"Postgres\".".to_owned(), false)]
    );
    drop((watch, phone, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_client_that_cannot_answer_leaves_the_question_to_the_away_wait() {
    let daemon = asking(TestDaemon::builder()).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    // NOTE: a follower with no `answers_questions`, as a piped `efr send`, efr 0.0.11
    // and the phone app of 0.0.11 subscribe.
    let before = daemon.clock().requested_sleeps().len();
    let (_, mut follow, _) = asked(&daemon, &client, false).await;
    pass_the_away_wait(&daemon, before).await;
    let events = until_end(&mut follow).await;

    // NOTE: the user is there; only no client that can answer follows.
    assert_eq!(end(&events), Some((UserAnswer::Unanswered { away: false }, None)));
    let read = outputs(&events);
    assert_eq!(read.len(), 1);
    assert!(read[0].0.starts_with("Nobody answered the question. "), "{read:?}");
    assert!(read[0].1);
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn an_interrupt_ends_the_question() {
    let daemon = asking(TestDaemon::builder()).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (sent, mut follow, _) = asked(&daemon, &client, true).await;
    let interrupt = Method::TurnInterrupt(TurnInterrupt {
        command_id: command_id(2),
        conversation_id: sent.conversation_id,
        turn_id: Some(sent.turn_id),
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    });
    let _: TurnInterruptResult = client.call(interrupt).await.unwrap();
    let events = until_end(&mut follow).await;

    assert_eq!(end(&events), Some((UserAnswer::Interrupted, None)));
    assert_eq!(kinds(&events).last(), Some(&"turn_interrupted"));
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_restart_ends_the_waiting_question() {
    let mut daemon = asking(TestDaemon::builder().persistent()).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let (sent, follow, question_id) = asked(&daemon, &client, true).await;
    drop((follow, client));
    daemon.restart().await.unwrap();

    let client = daemon.client_for_tty(TTY).await.unwrap();
    let answer = answer_question(2, sent.conversation_id, question_id, QuestionReply::Skipped);
    assert_eq!(code(client.call::<QuestionRespondResult>(answer).await), ErrorCode::NotFound);
    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    let names = kinds(&events);
    assert!(names.contains(&"turn_cancelled"), "{names:?}");
    let read = outputs(&events);
    assert_eq!(read.len(), 1, "the open call gets its error result: {names:?}");
    assert!(read[0].1);
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn with_the_switch_off_the_model_cannot_ask() {
    let builder = TestDaemon::builder().config(|settings| settings.conversation.ask_user = false);
    let daemon = asking(builder).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult =
        client.call(daemon.prompt(1, "set up the store", TTY)).await.unwrap();
    let mut follow = daemon.follow_answering(&client, sent.conversation_id).await.unwrap();
    let events = until_end(&mut follow).await;

    assert!(!kinds(&events).contains(&"user_question_requested"));
    let read = outputs(&events);
    assert_eq!(read.len(), 1);
    assert!(read[0].1, "the call of a tool that the request does not offer fails: {read:?}");
    drop((follow, client));
    daemon.stop().await.unwrap();
}
