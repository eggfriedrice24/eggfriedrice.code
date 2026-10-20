use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ClientFrame, ConversationId, ConversationResume, ConversationResumeResult, ConversationStatus,
    ConversationSummary, ConversationsListResult, ConversationsRecent, ConversationsRecentResult,
    ErrorBody, ErrorCode, Method, RecentConversation, RouteLabel, RouteScope, Seq,
};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::Target;
use crate::context::Context;
use crate::error::Exit;
use crate::keys::Keys;
use crate::run;
use crate::testing::{ScriptedKeys, TestEnv, capture, command, now};

const TTY: &str = "/dev/pts/7";

fn id(n: u64) -> ConversationId {
    format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse().unwrap()
}

fn summary(n: u64, title: &str) -> ConversationSummary {
    ConversationSummary {
        id: id(n),
        title: Some(title.to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(n),
        cwd: None,
        scope: None,
        tty: None,
    }
}

fn recent() -> ConversationsRecentResult {
    let row = |n, title| RecentConversation {
        summary: summary(n, title),
        route: RouteScope::Outside,
        project: None,
        turns: 1,
        open_in: None,
        here: false,
    };
    ConversationsRecentResult {
        scope: None,
        current: None,
        conversations: vec![row(1, "fix the tests"), row(2, "why is the build slow")],
    }
}

fn resumed(n: u64) -> ConversationResumeResult {
    ConversationResumeResult::Resumed {
        conversation_id: id(n),
        scope: RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from("/srv") },
        title: Some("why is the build slow".to_owned()),
        updated_at: now(),
        shell_dir: None,
        here: false,
        continues_here: true,
    }
}

fn held() -> ConversationResumeResult {
    ConversationResumeResult::Held {
        tty: "/dev/pts/4".to_owned(),
        title: Some("why is the build slow".to_owned()),
    }
}

/// A context of a terminal whose plugin handed over its shell context.
fn terminal(env: &TestEnv) -> Context {
    let context = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    Context { env: Env::fixed(vec![(Var::Context, context)]), ..env.context() }
}

/// What a run of `efr resume <args>` did.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    recent: Vec<ConversationsRecent>,
    resumes: Vec<ConversationResume>,
}

/// Runs `efr resume <args>` against a daemon that lists [`recent`], lists `listed` for
/// `conversations.list`, and answers each `conversation.resume` with the next of
/// `answers`. With `keys`, the test presses `key` after each `held` answer.
async fn resume_with(
    ctx: Context,
    env: &TestEnv,
    args: &[&str],
    listed: Vec<ConversationSummary>,
    answers: Vec<Result<ConversationResumeResult, ErrorBody>>,
    keys: Option<(Arc<ScriptedKeys>, u8)>,
) -> Ran {
    let daemon = env.listen();
    let ctx = match &keys {
        Some((keys, _)) => Context { keys: Arc::clone(keys) as Arc<dyn Keys>, ..ctx },
        None => ctx,
    };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut seen = (Vec::new(), Vec::new());
        let mut answers = answers.into_iter();
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsRecent(params) => {
                    seen.0.push(params);
                    conn.reply(id, &recent()).await;
                }
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: listed.clone(),
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::ConversationResume(params) => {
                    seen.1.push(params);
                    match answers.next().expect("an answer for each resume") {
                        Ok(result) => {
                            let is_held = matches!(result, ConversationResumeResult::Held { .. });
                            conn.reply(id, &result).await;
                            if let (true, Some((keys, key))) = (is_held, &keys) {
                                keys.press(*key).await;
                            }
                        }
                        Err(body) => conn.fail(id, body).await,
                    }
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        seen
    };
    let mut line = vec!["resume"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, (recent, resumes)) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), recent, resumes }
}

#[test]
fn resume_two_digits_is_n_and_longer_is_an_id_start() {
    assert_eq!(Target::of("2"), Target::Row(2));
    assert_eq!(Target::of("12"), Target::Row(12));
    assert_eq!(Target::of("123"), Target::Id("123".to_owned()));
    assert_eq!(Target::of("019a"), Target::Id("019a".to_owned()));
    assert_eq!(Target::of("+1"), Target::Id("+1".to_owned()));
}

#[tokio::test]
async fn resume_n_resumes_row_n_of_the_list_that_efrd_computes() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["2"], Vec::new(), vec![Ok(resumed(2))], None))
            .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [asked] = ran.recent.try_into().unwrap();
    assert_eq!((asked.tty.as_deref(), asked.pwd), (Some(TTY), Some(PathBuf::from("/srv"))));
    let [resume] = ran.resumes.try_into().unwrap();
    assert_eq!((resume.conversation_id, resume.take), (id(2), false));
    assert_eq!(resume.context.shell_pid, Some(4242));
    assert_eq!(ran.stdout, "↪ resumed \"why is the build slow\" (no project, just now)\n");
}

#[tokio::test]
async fn resume_lists_with_the_plugins_hint() {
    let env = TestEnv::new();
    let ran = Box::pin(resume_with(terminal(&env), &env, &[], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.stdout.starts_with(" 1  fix the tests"), "{}", ran.stdout);
    assert!(ran.stdout.ends_with(",resume N continues one here\n"), "{}", ran.stdout);
}

#[tokio::test]
async fn resume_of_a_row_that_is_not_there_fails() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["9"], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.stderr.contains("has no row 9; it has 2"), "{}", ran.stderr);
}

#[tokio::test]
async fn resume_ambiguous_id_start_fails_with_the_matches() {
    let env = TestEnv::new();
    let listed = vec![summary(1, "fix the tests"), summary(2, "why is the build slow")];
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["019a9b1c"], listed, Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.resumes.is_empty());
    assert!(
        ran.stderr.contains(&format!(
            "\"019a9b1c\" matches 2 conversations: {} (fix the tests), {} (why is the build slow); type more of the id",
            id(1),
            id(2)
        )),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn resume_held_asks_and_n_moves_nothing() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["2"],
        Vec::new(),
        vec![Ok(held())],
        Some((Arc::clone(&keys), b'n')),
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.resumes.len(), 1, "n sends nothing more");
    assert_eq!(
        ran.stdout,
        "\"why is the build slow\" is open in /dev/pts/4. Move it here? [y/N]\n"
    );
    assert_eq!(ran.stderr, "nothing moved\n");
}

#[tokio::test]
async fn resume_held_asks_and_y_takes_it() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["2"],
        Vec::new(),
        vec![Ok(held()), Ok(resumed(2))],
        Some((Arc::clone(&keys), b'y')),
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success);
    let [first, second] = ran.resumes.try_into().unwrap();
    assert!(!first.take && second.take);
    assert_ne!(first.command_id, second.command_id, "a new command, so no receipt answers it");
    assert!(ran.stdout.ends_with("↪ resumed \"why is the build slow\" (no project, just now)\n"));
}

#[tokio::test]
async fn resume_held_in_a_pipe_needs_take() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["2"], Vec::new(), vec![Ok(held())], None))
            .await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(
        ran.stderr.contains(
            "\"why is the build slow\" is open in /dev/pts/4; efr resume --take 2 moves it"
        ),
        "{}",
        ran.stderr
    );

    let env = TestEnv::new();
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["--take", "2"],
        Vec::new(),
        vec![Ok(resumed(2))],
        None,
    ))
    .await;
    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.resumes[0].take);
}

#[tokio::test]
async fn resume_on_an_older_efrd_says_restart() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let body = ErrorBody::new(ErrorCode::Invalid, "the frame is not a valid client frame");
        conn.fail(id, body).await;
    };
    let ctx = terminal(&env);
    let line = command(&["resume"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::DaemonError);
    assert!(
        captured
            .stderr()
            .contains("efrd is older than this efr; restart it: systemctl --user restart efrd"),
        "{}",
        captured.stderr()
    );
}

#[tokio::test]
async fn resume_of_a_conversation_needs_a_terminal() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(env.context(), &env, &["2"], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.stderr.contains("efr resume needs a terminal"), "{}", ran.stderr);
}
