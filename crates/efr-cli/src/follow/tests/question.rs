//! The keys of the model's question, against a fake daemon: a digit answers it, `t`
//! opens the answer line and Enter sends it, Esc skips it or goes back to the options,
//! and a byte of an escape sequence never answers it.

use std::sync::{Arc, Mutex};

use efr_protocol::{
    ConversationHistoryResult, ErrorBody, ErrorCode, Event, Method, Origin, QuestionOption,
    QuestionReply, QuestionRespond, QuestionRespondResult, RequestId, Seq, TurnId, UserAnswer,
};
use pretty_assertions::assert_eq;

use super::super::{Asking, QuestionKeys};
use super::{raw_view, run, run_view, subscription, turn_completed};
use crate::answer::{AnswerLine, Sequences};
use crate::context::Context;
use crate::testing::{
    Conn, ScriptedKeys, TestEnv, call, conversation, envelope, item, question_id, turn,
};

fn requested() -> Event {
    asked_in(turn())
}

/// The question, asked in turn `turn_id`.
fn asked_in(turn_id: TurnId) -> Event {
    let option = |label: &str| QuestionOption { label: label.to_owned(), description: None };
    Event::UserQuestionRequested {
        turn_id,
        call_id: call(),
        question_id: question_id(),
        question: "Which database should the tests use?".to_owned(),
        options: vec![option("Postgres 16"), option("SQLite"), option("both")],
    }
}

/// What the user does at the question.
#[derive(Debug, Clone, Copy)]
enum Press {
    /// These bytes, one key each.
    Bytes(&'static [u8]),
    /// Esc alone.
    Esc,
}

/// Reads the next request, which must be `question.respond` for the question.
async fn responded(conn: &mut Conn) -> (RequestId, QuestionRespond) {
    let (id, method) = conn.request().await;
    let Method::QuestionRespond(params) = method else {
        panic!("expected question.respond, got {}", method.name());
    };
    assert_eq!(params.conversation_id, conversation());
    assert_eq!(params.question_id, question_id());
    (id, params)
}

/// Answers the question with `presses`, and returns the reply that went to efrd and
/// what the view wrote to stderr.
async fn answer_with(presses: &'static [Press]) -> (QuestionReply, String) {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let sent: Arc<Mutex<Option<QuestionReply>>> = Arc::default();
    let seen = Arc::clone(&sent);
    let (result, _, err) = Box::pin(run(&env, &ctx, |mut conn, _| async move {
        let (sub, params) = subscription(&mut conn, 10).await;
        assert!(params.answers_questions, "keys can be read, so a person here can answer");
        conn.item(sub, &item(11, requested())).await;
        for press in presses {
            match press {
                Press::Bytes(bytes) => keys.type_bytes(bytes).await,
                Press::Esc => keys.press_esc().await,
            }
        }
        let (id, params) = responded(&mut conn).await;
        conn.reply(id, &QuestionRespondResult { seq: Seq::new(12) }).await;
        let answered = Event::UserQuestionAnswered {
            turn_id: turn(),
            question_id: question_id(),
            answer: UserAnswer::Replied { reply: params.answer.clone() },
            origin: Some(Origin::Shell),
        };
        *seen.lock().unwrap() = Some(params.answer);
        conn.item(sub, &item(12, answered)).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    }))
    .await;
    result.unwrap();
    let reply = sent.lock().unwrap().take().expect("an answer went to efrd");
    (reply, err)
}

#[tokio::test]
async fn a_digit_answers_with_its_option() {
    let (reply, err) = answer_with(&[Press::Bytes(b"2")]).await;
    assert_eq!(reply, QuestionReply::Option { index: 1 });
    assert!(err.ends_with("\u{2713} answered \u{b7} SQLite\n"), "{err}");
}

#[tokio::test]
async fn a_byte_of_an_escape_sequence_or_a_digit_past_the_options_answers_nothing() {
    // F5 holds a 1, Delete a 3; there is no option 9.
    let (reply, _) = answer_with(&[Press::Bytes(b"\x1b[15~\x1b[3~9x3")]).await;
    assert_eq!(reply, QuestionReply::Option { index: 2 });
}

#[tokio::test]
async fn t_opens_the_answer_line_and_enter_sends_what_it_holds() {
    let presses = &[Press::Bytes(b"t"), Press::Bytes(b"\r12 on port 5433\x7f3\r")];
    let (reply, err) = answer_with(presses).await;
    // The digits typed into the line are text, and an empty Enter sends nothing.
    assert_eq!(reply, QuestionReply::Text { text: "12 on port 5433".to_owned() });
    assert!(err.contains("Do not type a password or a token here."), "{err}");
    assert!(err.ends_with("\u{2713} answered \u{b7} \"12 on port 5433\"\n"), "{err}");
}

#[tokio::test]
async fn esc_skips_and_at_the_answer_line_goes_back_to_the_options() {
    let (reply, err) = answer_with(&[Press::Esc]).await;
    assert_eq!(reply, QuestionReply::Skipped);
    assert!(
        err.ends_with(
            "skipped \u{b7} the model goes on with a choice that is safe and easy to undo\n"
        ),
        "{err}"
    );
    let presses = &[Press::Bytes(b"tpg"), Press::Esc, Press::Bytes(b"1")];
    let (reply, _) = answer_with(presses).await;
    assert_eq!(reply, QuestionReply::Option { index: 0 });
}

#[tokio::test]
async fn an_answer_that_efrd_did_not_take_is_a_note() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let (sub, _) = subscription(&mut conn, 10).await;
        conn.item(sub, &item(11, requested())).await;
        keys.press(b'1').await;
        let (id, _) = responded(&mut conn).await;
        conn.fail(id, ErrorBody::new(ErrorCode::NotFound, "no such question waits")).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(err.ends_with("the answer was not taken: no such question waits\n"), "{err}");
}

#[tokio::test]
async fn without_keys_nobody_here_answers_the_question() {
    let env = TestEnv::new();
    let ctx = env.context();
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let (sub, params) = subscription(&mut conn, 10).await;
        assert!(!params.answers_questions, "no terminal on stdin, so nobody here can answer");
        conn.item(sub, &item(11, requested())).await;
        let unanswered = Event::UserQuestionAnswered {
            turn_id: turn(),
            question_id: question_id(),
            answer: UserAnswer::Unanswered { away: false },
            origin: None,
        };
        conn.item(sub, &item(12, unanswered)).await;
        conn.item(sub, &item(13, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(err.contains("waiting for another client to answer\n"), "{err}");
    assert!(err.contains("\u{2717} nobody answered"), "{err}");
}

#[tokio::test]
async fn a_queued_prompt_shows_and_answers_the_question_that_the_running_turn_waits_for() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let mut view = raw_view();
    view.queue();
    let running: TurnId = "0192f0c1-7a00-7000-8000-000000000077".parse().unwrap();
    let (result, _, err) = Box::pin(run_view(&env, &ctx, view, |mut conn, _| async move {
        // The running turn asked before the prompt queued: the newest page of the log
        // has the question, and an earlier one that ended.
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationHistory(_)), "{}", method.name());
        let ended = Event::UserQuestionAnswered {
            turn_id: running,
            question_id: "019a9b1c-3d00-7a10-8b20-0000000000e9".parse().unwrap(),
            answer: UserAnswer::Unanswered { away: false },
            origin: None,
        };
        let mut earlier = asked_in(running);
        if let Event::UserQuestionRequested { question_id, .. } = &mut earlier {
            *question_id = "019a9b1c-3d00-7a10-8b20-0000000000e9".parse().unwrap();
        }
        let page = ConversationHistoryResult {
            events: vec![envelope(6, earlier), envelope(7, ended), envelope(8, asked_in(running))],
            next_cursor: None,
        };
        conn.reply(id, &page).await;
        let (sub, _) = subscription(&mut conn, 10).await;
        keys.press(b'3').await;
        let (id, params) = responded(&mut conn).await;
        assert_eq!(params.answer, QuestionReply::Option { index: 2 });
        conn.reply(id, &QuestionRespondResult { seq: Seq::new(11) }).await;
        let cancelled = Event::TurnCancelled { turn_id: turn() };
        conn.item(sub, &item(11, cancelled)).await;
        conn.until_closed().await;
    }))
    .await;
    assert!(result.is_err(), "the followed turn was cancelled");
    assert_eq!(err.matches(" ?  Which database should the tests use?").count(), 1, "{err}");
    assert!(err.contains("\u{2713} answered \u{b7} both\n"), "{err}");
}

#[test]
fn an_open_answer_line_throws_its_unread_keys_away() {
    let keys = |line: Option<AnswerLine>| {
        Asking::Question(QuestionKeys {
            question_id: question_id(),
            options: 3,
            line,
            sequences: Sequences::default(),
        })
    };
    assert!(!keys(None).guards(), "a digit or Esc can go to the row");
    assert!(
        keys(Some(AnswerLine::new())).guards(),
        "the rest of an answer never goes to the row, where Enter steers"
    );
}
