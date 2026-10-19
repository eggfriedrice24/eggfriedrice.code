//! The `a` key against a fake daemon (phase E): it asks for a confirm, only `y` there
//! sends `allow_always`, any other key asks the question again, and keys typed before
//! the confirm showed never answer it. A question that offers no rule, and the
//! quarantine question, ignore `a`.

use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    AlwaysSaved, ApprovalDecision, ApprovalRespondResult, ErrorBody, ErrorCode, Event, ExitInfo,
    ExitKind, Grant, Launch, Method, Origin, QuestionId, SandboxSurfaceRespondResult, Seq,
    SurfaceChange,
};
use pretty_assertions::assert_eq;

use super::{Stream, run, shows_on, subscribed, turn_completed};
use crate::context::Context;
use crate::testing::{Conn, ScriptedKeys, TestEnv, call, conversation, exit_info, item, turn};

/// The rule that the test's question offers.
const RULE: &str = "write under ~/notes";

/// An exit question for a write to `~/notes`, which offers `always`.
fn write_question(always: Option<&str>) -> Event {
    let grants = vec![Grant::Write { path: PathBuf::from("/home/user/notes") }];
    Event::ApprovalRequested {
        turn_id: turn(),
        call_id: call(),
        summary: "write ~/notes/x".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: Some(ExitInfo {
            always: always.map(str::to_owned),
            ..exit_info(&[ExitKind::Write], Launch::Contained { grants })
        }),
    }
}

/// Reads the next request, an `approval.respond` of the test's call, and returns its id
/// and decision.
async fn respond(conn: &mut Conn) -> (efr_protocol::RequestId, ApprovalDecision) {
    let (id, method) = conn.request().await;
    let Method::ApprovalRespond(params) = method else {
        panic!("expected approval.respond, got {}", method.name());
    };
    assert_eq!(params.conversation_id, conversation());
    assert_eq!(params.call_id, call());
    (id, params.decision)
}

/// Waits until stderr holds `text` `count` times.
async fn shown(seen: &crate::testing::Captured, text: &str, count: usize) {
    shows_on(seen, Stream::Stderr, |err| err.matches(text).count() == count).await;
}

#[tokio::test]
async fn a_key_needs_confirm() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::threaded());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, err) = run(&env, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, write_question(Some(RULE)))).await;
        shown(&seen, "a always: write under ~/notes", 1).await;
        // `a` asks first. The keys typed before the confirm showed answer nothing.
        presser.press(b'a').await;
        shown(&seen, "save rule", 1).await;
        presser.press(b'y').await;
        presser.marked().await;
        // Any other key goes back to the question, with no answer.
        presser.press(b'x').await;
        shown(&seen, "a always: write under ~/notes", 2).await;
        presser.press(b'a').await;
        shown(&seen, "save rule", 2).await;
        presser.marked().await;
        presser.press(b'y').await;
        let (id, decision) = respond(&mut conn).await;
        assert_eq!(decision, ApprovalDecision::AllowAlways);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        let resolved = Event::ApprovalResolved {
            turn_id: turn(),
            call_id: call(),
            decision: ApprovalDecision::AllowAlways,
            origin: Origin::Cli,
        };
        conn.item(sub, &item(12, resolved)).await;
        let saved = Event::AlwaysRuleSaved {
            turn_id: turn(),
            call_id: call(),
            outcome: AlwaysSaved::Saved { index: 0, rule: RULE.to_owned() },
        };
        conn.item(sub, &item(13, saved)).await;
        conn.item(sub, &item(14, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(keys.starts(), 1);
    insta::assert_snapshot!(err);
}

#[tokio::test]
async fn an_always_answer_the_daemon_refuses_asks_the_question_again() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::threaded());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, err) = run(&env, &ctx, |mut conn, seen| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, write_question(Some(RULE)))).await;
        shown(&seen, "a always", 1).await;
        presser.press(b'a').await;
        shown(&seen, "save rule", 1).await;
        presser.marked().await;
        presser.press(b'y').await;
        let (id, decision) = respond(&mut conn).await;
        assert_eq!(decision, ApprovalDecision::AllowAlways);
        let body = ErrorBody::new(ErrorCode::Forbidden, "only your own terminal saves a rule");
        conn.fail(id, body).await;
        shown(&seen, "the answer was not taken", 1).await;
        // The question stays open, without the `a`: `a` does nothing now, `y` allows.
        presser.press(b'a').await;
        presser.press(b'y').await;
        let (id, decision) = respond(&mut conn).await;
        assert_eq!(decision, ApprovalDecision::Allow);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert_eq!(err.matches("save rule").count(), 1, "{err}");
    assert!(
        err.contains(
            "\nthe answer was not taken: only your own terminal saves a rule\n\
             \u{2502} y allow \u{b7} n deny\n\n\u{2713} allowed\n"
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_key_ignored_without_offer() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        conn.item(sub, &item(11, write_question(None))).await;
        presser.press(b'a').await;
        presser.press(b'y').await;
        let (id, decision) = respond(&mut conn).await;
        assert_eq!(decision, ApprovalDecision::Allow);
        conn.reply(id, &ApprovalRespondResult { seq: Seq::new(12) }).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!err.contains("always"), "{err}");
    assert!(!err.contains("save rule"), "{err}");
    assert!(err.ends_with("\u{2502} y allow \u{b7} n deny\n\u{2713} allowed\n"), "{err}");
}

#[tokio::test]
async fn a_key_ignored_at_quarantine_question() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = Context { keys: keys.clone(), ..env.context() };
    let presser = Arc::clone(&keys);
    let question: QuestionId = "019a9b1c-3d00-7a10-8b20-0000000000d1".parse().unwrap();
    let (result, _, err) = run(&env, &ctx, |mut conn, _| async move {
        let sub = subscribed(&mut conn, 10).await;
        let request = Event::SurfaceQuestionRequested {
            turn_id: turn(),
            call_id: call(),
            question_id: question,
            changes: vec![SurfaceChange {
                path: PathBuf::from("/p/app/.git/config"),
                rule: "config_key".to_owned(),
                key: Some("core.fsmonitor".to_owned()),
                quarantined: true,
            }],
        };
        conn.item(sub, &item(11, request)).await;
        // The quarantine question keeps its own keys: `a` answers nothing there.
        presser.press(b'a').await;
        presser.press(b'n').await;
        let (id, method) = conn.request().await;
        let Method::SandboxSurfaceRespond(params) = method else {
            panic!("expected sandbox.surface_respond, got {}", method.name());
        };
        assert_eq!(params.question_id, question);
        assert!(!params.keep);
        conn.reply(id, &SandboxSurfaceRespondResult { seq: Seq::new(12) }).await;
        conn.item(sub, &item(12, turn_completed())).await;
        conn.until_closed().await;
    })
    .await;
    result.unwrap();
    assert!(!err.contains("save rule"), "{err}");
    assert!(
        err.ends_with(
            "\u{2502} y keep \u{b7} n leave in quarantine\n\u{2717} left in quarantine\n"
        ),
        "{err}"
    );
}
