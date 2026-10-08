use std::sync::Arc;

use efr_protocol::{
    ClientFrame, CompactionTrigger, ConversationCompact, ConversationCompactResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, ErrorBody, ErrorCode, Method,
    Seq,
};
use efr_render::{ColourMode, RenderOptions};
use efr_stdx::env::{Env, Var};
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::row;
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{
    TestEnv, TestInterrupt, capture, command, compaction, conversation, now, readable,
    terminal_facts,
};

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: conversation(),
        title: Some("run the tests".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: None,
    }
}

fn result() -> ConversationCompactResult {
    ConversationCompactResult {
        seq: Seq::new(10),
        compaction: compaction(CompactionTrigger::Manual, 19_000),
    }
}

/// What a run of `efr compact <args>` did against a daemon that lists `listed` and
/// answers `conversation.compact` with `answer`.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    asked: Option<ConversationCompact>,
}

/// Runs `efr compact <args>` with `ctx` against a daemon that lists `listed` and
/// answers `conversation.compact` with `answer`.
async fn compact_with(
    env: &TestEnv,
    ctx: Context,
    args: &[&str],
    answer: Result<ConversationCompactResult, ErrorBody>,
    listed: Vec<ConversationSummary>,
) -> Ran {
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut asked = None;
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: listed.clone(),
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::ConversationCompact(params) => {
                    asked = Some(params);
                    match &answer {
                        Ok(result) => conn.reply(id, result).await,
                        Err(body) => conn.fail(id, body.clone()).await,
                    }
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let mut line = vec!["compact"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), asked }
}

#[tokio::test]
async fn efr_compact_compacts_the_newest_conversation_and_says_what_came_of_it() {
    let env = TestEnv::new();
    let ran = Box::pin(compact_with(&env, env.context(), &[], Ok(result()), vec![summary()])).await;
    assert_eq!(ran.exit, Exit::Success);
    let asked = ran.asked.unwrap();
    assert_eq!((asked.conversation_id, asked.focus), (conversation(), None));
    assert_eq!(
        ran.stdout,
        "context compacted (efr compact): 231k -> 19k tokens, kept 3 turns, summary 3.2k\n"
    );
    assert_eq!(
        ran.stderr,
        "the newest conversation; efr compact --conversation <id> compacts another\n"
    );
}

#[tokio::test]
async fn the_focus_comes_from_the_words_else_from_the_plugins_variable() {
    let env = TestEnv::new();
    let id = conversation().to_string();
    let words = ["--conversation", &id, "the", "failing", "test"];
    let ran = Box::pin(compact_with(&env, env.context(), &words, Ok(result()), Vec::new())).await;
    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.asked.unwrap().focus.as_deref(), Some("the failing test"));
    assert_eq!(ran.stderr, "", "a named conversation needs no line about it");

    let env = TestEnv::new();
    let vars = Env::fixed([(Var::Prompt, "  the fix; keep 'quotes' * ")]);
    let ctx = Context { env: vars, ..env.context() };
    let ran =
        Box::pin(compact_with(&env, ctx, &["--conversation", &id], Ok(result()), Vec::new())).await;
    assert_eq!(ran.asked.unwrap().focus.as_deref(), Some("the fix; keep 'quotes' *"));

    let env = TestEnv::new();
    let ctx = Context { env: Env::fixed([(Var::Prompt, "   ")]), ..env.context() };
    let ran =
        Box::pin(compact_with(&env, ctx, &["--conversation", &id], Ok(result()), Vec::new())).await;
    assert_eq!(ran.asked.unwrap().focus, None, "a blank focus is none");
}

#[tokio::test]
async fn a_refusal_of_the_daemon_is_the_error() {
    let env = TestEnv::new();
    let body = ErrorBody::new(ErrorCode::Conflict, "a turn of the conversation runs");
    let ran = Box::pin(compact_with(&env, env.context(), &[], Err(body), vec![summary()])).await;
    assert_eq!(ran.exit, Exit::DaemonError);
    assert_eq!(ran.stdout, "");
    assert!(
        ran.stderr.ends_with(
            "efr: the daemon failed the request with conflict: a turn of the conversation runs\n"
        ),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn without_a_conversation_there_is_nothing_to_compact() {
    let env = TestEnv::new();
    let ran = Box::pin(compact_with(&env, env.context(), &[], Ok(result()), Vec::new())).await;
    assert_eq!(ran.exit, Exit::DaemonError);
    assert_eq!(ran.asked, None);
    assert_eq!(
        ran.stderr,
        "efr: there is no conversation yet, so there is no context to compact\n"
    );
}

#[tokio::test]
async fn on_a_terminal_a_status_row_waits_and_goes_before_the_muted_line() {
    let env = TestEnv::new();
    let ctx = Context { term: terminal_facts(), ..env.context() };
    let ran = Box::pin(compact_with(&env, ctx, &[], Ok(result()), vec![summary()])).await;
    assert_eq!(ran.exit, Exit::Success);
    insta::assert_snapshot!(readable(&ran.stdout));
}

#[test]
fn the_status_row_turns_its_spinner_and_shows_the_time_from_one_second() {
    let options = RenderOptions::new(40).with_colour(ColourMode::None);
    let at = |millis| now() + SignedDuration::from_millis(millis);
    let rows: Vec<String> = [(0, 0), (1, 100), (12, 1_200), (100, 61_000)]
        .into_iter()
        .map(|(ticks, millis)| readable(&row(true, ticks, now(), at(millis), &options)))
        .collect();
    assert_eq!(
        rows,
        [
            "\\e[1m\u{280b}\\e[0m \\e[2mcompacting context\\e[0m",
            "\\e[1m\u{2819}\\e[0m \\e[2mcompacting context\\e[0m",
            "\\e[1m\u{2839}\\e[0m \\e[2mcompacting context\\e[0m  \\e[2m1s\\e[0m",
            "\\e[1m\u{280b}\\e[0m \\e[2mcompacting context\\e[0m  \\e[2m1m 01s\\e[0m",
        ]
    );
    let still = row(false, 7, now(), now(), &options);
    assert!(still.contains('\u{2022}'), "{still:?}");
    let narrow = readable(&row(true, 0, now(), at(5_000), &RenderOptions::new(16)));
    assert!(narrow.contains("compactin\u{2026}") && narrow.contains("5s"), "{narrow}");
}

#[tokio::test]
async fn ctrl_c_stops_the_wait_and_says_that_efrd_may_go_on() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context { interrupt: interrupt.clone(), ..env.context() };
    let (mut out, captured) = capture();
    let id = conversation().to_string();
    let script = async {
        let mut conn = daemon.accept().await;
        let (_, method) = conn.request().await;
        assert!(matches!(method, Method::ConversationCompact(_)), "{method:?}");
        interrupt.trigger();
        conn.until_closed().await;
    };
    let line = command(&["compact", "--conversation", &id]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Interrupted);
    assert_eq!(captured.stdout(), "");
    assert_eq!(captured.stderr(), "stopped waiting; efrd may still compact the conversation\n");
}
