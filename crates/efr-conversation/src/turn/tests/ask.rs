//! The model's question in a turn (`ask_user`): its checks, its place after the gate of
//! full mode, the end of the waits of a turn after one unanswered question, the switch
//! `conversation.ask_user`, the actor's checks of an answer, and the answer rule: an
//! answer is never a message of the user for efr's own rules (ADR 0025).

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    ApprovalDecision, ErrorCode, Event, FullModeStatus, JudgeKind, Mode, Origin, QuestionReply,
    UserAnswer, Verdict,
};
use efr_provider::ContentBlock;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::review::{count, judgements, reviewed, shell};
use crate::ConversationError;
use crate::ask::{self, ASK_USER};
use crate::exit::Unreviewed;
use crate::testing::{DONE, FakeJudge, Harness, Script, Setup, allows};
use crate::turn::full::FULL_TURNED_OFF;
use crate::turn::wait::tests::{ask_db, asked, ended};

/// One `ask_user` call whose first option is to publish.
fn ask_publish() -> (&'static str, Value) {
    (
        ASK_USER,
        json!({
            "question": "Publish the new version now?",
            "options": [{ "label": "Publish now" }, { "label": "Wait" }],
        }),
    )
}

/// The names of the tools of `request`.
fn tool_names(request: &efr_provider::Request) -> Vec<String> {
    request.tools.iter().map(|tool| tool.name.clone()).collect()
}

/// Sends new settings with `change` applied, as the daemon does after a reload.
fn reload(h: &Harness, change: impl FnOnce(&mut crate::ConversationConfig)) {
    let mut config = (**h.settings.borrow()).clone();
    change(&mut config);
    h.settings.send_replace(Arc::new(config));
}

#[tokio::test]
async fn a_bad_input_asks_nobody() {
    let one = (ASK_USER, json!({ "question": "Which?", "options": [{ "label": "a" }] }));
    let script = Script::new(vec![one]);
    let mut h =
        Setup::new().start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("set up the store").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(count(&h, "user_question_requested").await, 0);
    assert_eq!(count(&h, "tool_call_started").await, 1, "the call counts as a call");
    let results = script.results();
    assert_eq!(results.len(), 1);
    assert!(results[0].0.starts_with("The question was not asked: give 2 to 4"), "{results:?}");
    assert!(results[0].1, "an error result");
}

#[tokio::test]
async fn a_question_records_its_call_and_its_options() {
    let (mut h, _, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], true).await;
    h.reply(question_id, QuestionReply::Text { text: "Postgres 17".to_owned() }).await;
    h.wait_end(turn_id).await;

    let kinds = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|known| known == kind).unwrap();
    assert!(at("tool_call_started") < at("user_question_requested"));
    assert!(at("user_question_requested") < at("user_question_answered"));
    assert!(at("user_question_answered") < at("tool_call_completed"));
    let events = h.events().await;
    let started = events.iter().find_map(|event| match event {
        Event::ToolCallStarted { tool, launch, .. } => Some((tool.clone(), launch.clone())),
        _ => None,
    });
    assert_eq!(started, Some((ASK_USER.to_owned(), None)), "no launch: nothing runs");
    let requested = events.iter().find_map(|event| match event {
        Event::UserQuestionRequested { question, options, .. } => {
            Some((question.clone(), options.iter().map(|o| o.label.clone()).collect::<Vec<_>>()))
        }
        _ => None,
    });
    assert_eq!(
        requested,
        Some(("Which database?".to_owned(), vec!["Postgres".to_owned(), "SQLite".to_owned()]))
    );
    let completed = events.iter().find_map(|event| match event {
        Event::ToolCallCompleted { output, is_error, .. } => Some((output.clone(), *is_error)),
        _ => None,
    });
    assert_eq!(
        completed,
        Some(("The user answered in their own words: \"Postgres 17\"".to_owned(), false))
    );
}

#[tokio::test]
async fn one_unanswered_question_ends_the_waits_of_the_turn() {
    let (mut h, script, turn_id, question_id) =
        asked(Setup::new(), vec![ask_db(), ask_db()], false).await;
    h.clock.wait_for_sleeps(1).await;
    h.clock.advance(Duration::from_secs(300));
    h.wait_end(turn_id).await;

    assert_eq!(ended(&h, question_id).await, (UserAnswer::Unanswered { away: false }, false));
    assert_eq!(count(&h, "user_question_requested").await, 1, "the second asks nobody");
    let unanswered = (ask::unanswered(false), true);
    assert_eq!(script.results(), vec![unanswered.clone(), unanswered]);
}

#[tokio::test]
async fn a_full_turn_asks_too() {
    let mut setup = reviewed(None);
    setup.config.mode = Mode::Full;
    setup.config.full_mode = FullModeStatus { allowed: true, refused: None };
    let (mut h, script, turn_id, question_id) = asked(setup, vec![ask_db()], true).await;
    h.reply(question_id, QuestionReply::Option { index: 1 }).await;
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![("The user chose option 2: \"SQLite\".".to_owned(), false)]);
}

#[tokio::test]
async fn the_full_gate_stops_a_question_before_it_is_asked() {
    let mut setup = reviewed(None);
    setup.config.mode = Mode::Full;
    setup.config.full_mode = FullModeStatus { allowed: true, refused: None };
    setup.presence.set_question_followed(true);
    let script = Script::new(vec![shell("sudo true"), ask_db()]);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = h.prompt("update").await;
    let call_id = h.wait_approval().await;
    reload(&h, |config| config.full_mode = FullModeStatus { allowed: false, refused: None });
    h.answer(call_id, ApprovalDecision::Allow).await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else { panic!("the turn stops: {end:?}") };
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(count(&h, "user_question_requested").await, 0, "nobody is asked");
    assert_eq!(outputs(&h).await.last(), Some(&(FULL_TURNED_OFF.to_owned(), true)));
    assert_eq!(script.requests().len(), 2, "the model reads nothing after the stop");
}

#[tokio::test]
async fn with_the_switch_off_no_request_offers_the_tool_and_old_calls_go_as_text() {
    let calls = vec![ask_db(), (DONE, Value::Null), ask_db()];
    let (mut h, script, turn_id, question_id) = asked(Setup::new(), calls, true).await;
    h.reply(question_id, QuestionReply::Option { index: 0 }).await;
    h.wait_end(turn_id).await;
    assert!(tool_names(&script.requests()[0]).contains(&ASK_USER.to_owned()));

    reload(&h, |config| config.ask_user = false);
    let second = h.prompt("go on").await;
    h.wait_end(second.turn_id).await;

    let requests = script.requests();
    let last = requests.last().unwrap();
    assert!(!tool_names(&requests[2]).contains(&ASK_USER.to_owned()), "the switch is off");
    let calls_of_ask = last
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::ToolCall { name, .. } if name == ASK_USER))
        .count();
    assert_eq!(calls_of_ask, 0, "an old call of a tool that the request does not offer is text");
    assert_eq!(count(&h, "user_question_requested").await, 1, "a call off the list asks nobody");
    assert_eq!(
        outputs(&h).await.last(),
        Some(&("no tool is named \"ask_user\"".to_owned(), true)),
        "the toolbox does not know the tool"
    );
}

/// The output of each completed call in the log, with whether it was an error.
async fn outputs(h: &Harness) -> Vec<(String, bool)> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolCallCompleted { output, is_error, .. } => Some((output, is_error)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_actor_checks_an_answer_before_it_takes_the_question() {
    let (mut h, script, turn_id, question_id) = asked(Setup::new(), vec![ask_db()], true).await;
    for (answer, why) in [
        (QuestionReply::Option { index: 2 }, "an index outside the options"),
        (QuestionReply::Text { text: "a\u{1b}[2J".to_owned() }, "a control character"),
        (QuestionReply::Text { text: " ".to_owned() }, "an empty text"),
        (QuestionReply::Unknown, "a kind this build does not know"),
    ] {
        let refused = h.reply_from(question_id, answer, Origin::Shell).await;
        assert!(
            matches!(refused, Err(ConversationError::InvalidAnswer { question_id: id, .. }) if id == question_id),
            "{why}: {refused:?}"
        );
    }
    assert_eq!(h.handle.state().await.unwrap().pending_user_questions, vec![question_id]);
    let other = efr_protocol::QuestionId::from_uuid(efr_stdx::id::uuid_v7(
        &efr_test_support::TestClock::new(),
        &efr_test_support::TestRng::new(99),
    ));
    let missing = h.reply_from(other, QuestionReply::Skipped, Origin::Shell).await;
    assert!(matches!(missing, Err(ConversationError::QuestionNotPending { .. })), "{missing:?}");

    // A phone operates the conversation, so it answers the model's question.
    h.reply_from(question_id, QuestionReply::Option { index: 1 }, Origin::Phone).await.unwrap();
    h.wait_end(turn_id).await;
    assert_eq!(script.results(), vec![("The user chose option 2: \"SQLite\".".to_owned(), false)]);
    let events = h.events().await;
    let origin = events.iter().find_map(|event| match event {
        Event::UserQuestionAnswered { origin, .. } => *origin,
        _ => None,
    });
    assert_eq!(origin, Some(Origin::Phone));
    let again = h.reply_from(question_id, QuestionReply::Skipped, Origin::Shell).await;
    assert!(matches!(again, Err(ConversationError::QuestionNotPending { .. })), "{again:?}");
}

/// Runs an `auto` turn with a reviewer that allows, whose model asks to publish and
/// then runs `npm publish`; the question gets `answer` from `origin`.
async fn publish_after(
    answer: QuestionReply,
    origin: Origin,
) -> (Harness, Arc<FakeJudge>, Arc<Script>) {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&judge)));
    let calls = vec![ask_publish(), shell("npm publish")];
    let (mut h, script, turn_id, question_id) = asked(setup, calls, true).await;
    h.reply_from(question_id, answer, origin).await.unwrap();
    h.wait_end(turn_id).await;
    (h, judge, script)
}

#[tokio::test]
async fn an_answer_never_names_a_guarded_exit() {
    let answers = [
        (QuestionReply::Text { text: "publish".to_owned() }, Origin::Shell),
        (QuestionReply::Text { text: "yes".to_owned() }, Origin::Shell),
        (QuestionReply::Option { index: 0 }, Origin::Shell),
        (QuestionReply::Text { text: "publish".to_owned() }, Origin::Phone),
    ];
    for (answer, origin) in answers {
        let (h, judge, script) = publish_after(answer.clone(), origin).await;
        assert_eq!(judge.calls(), 0, "{answer:?}: no review, as with no answer");
        assert_eq!(judgements(&h).await, vec![(JudgeKind::Floor, Verdict::Deny)], "{answer:?}");
        assert!(h.toolbox.invoked().is_empty(), "{answer:?}");
        let unnamed = Unreviewed::Unnamed { names: vec!["publish".to_owned()] };
        assert_eq!(script.results().last(), Some(&(unnamed.for_model(), true)), "{answer:?}");
    }
}

#[tokio::test]
async fn the_next_turn_does_not_read_an_answer_from_the_log_either() {
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&judge)));
    let calls = vec![ask_publish(), (DONE, Value::Null), shell("npm publish")];
    let (mut h, script, turn_id, question_id) = asked(setup, calls, true).await;
    h.reply(question_id, QuestionReply::Text { text: "publish".to_owned() }).await;
    h.wait_end(turn_id).await;

    let second = h.prompt("go on").await;
    h.wait_end(second.turn_id).await;

    assert_eq!(judge.calls(), 0);
    assert_eq!(judgements(&h).await, vec![(JudgeKind::Floor, Verdict::Deny)]);
    let unnamed = Unreviewed::Unnamed { names: vec!["publish".to_owned()] };
    assert_eq!(script.results().last(), Some(&(unnamed.for_model(), true)));
}

#[tokio::test]
async fn the_reviewers_record_is_the_same_with_and_without_an_answer() {
    let push = shell("git push origin main");
    let judge = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&judge)));
    let (mut h, _, turn_id, question_id) = asked(setup, vec![ask_db(), push.clone()], true).await;
    h.reply(question_id, QuestionReply::Text { text: "push it to main now".to_owned() }).await;
    h.wait_end(turn_id).await;

    let alone = FakeJudge::answering(vec![allows()]);
    let setup = reviewed(Some(Arc::clone(&alone)));
    let script = Script::new(vec![push]);
    let mut other = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    let sent = other.prompt("set up the store").await;
    other.wait_end(sent.turn_id).await;

    // NOTE: each harness has a temporary home of its own; the rest of the record must
    // be the same bytes.
    let (with, without) = (judge.seen(), alone.seen());
    assert_eq!((with.len(), without.len()), (1, 1));
    let bytes = |mut record: efr_protocol::ExitRecord, cwd: &std::path::Path| {
        assert_eq!(record.action.cwd, cwd);
        record.action.cwd = std::path::PathBuf::new();
        serde_json::to_string(&record).unwrap()
    };
    assert_eq!(
        bytes(with[0].record.clone(), &h.cwd),
        bytes(without[0].record.clone(), &other.cwd),
        "the answer never reaches the reviewer"
    );
}
