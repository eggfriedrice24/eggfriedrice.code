use std::time::Duration;

use efr_protocol::{
    ClientFrame, ConversationStatus, ConversationSummary, ConversationsListResult, Method,
    Presence, PresenceSet, PresenceSetResult, Seq,
};
use pretty_assertions::assert_eq;

use super::away_line;
use crate::context::Context;
use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, conversation, now};

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

/// What a run of `efr away <args>` did.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    asked: Option<PresenceSet>,
}

/// Runs `efr away <args>` with `ctx` against a daemon that lists `listed` and answers
/// `presence.set` with `changed` conversations.
async fn away_with(
    env: &TestEnv,
    ctx: Context,
    args: &[&str],
    changed: bool,
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
                Method::PresenceSet(params) => {
                    let result = PresenceSetResult {
                        presence: params.presence,
                        changed: params.conversation_id.filter(|_| changed).into_iter().collect(),
                        seq: changed.then(|| Seq::new(10)),
                    };
                    asked = Some(params);
                    conn.reply(id, &result).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let mut line = vec!["away"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), asked }
}

#[tokio::test]
async fn efr_away_sends_presence_set() {
    let env = TestEnv::new();
    let ran = Box::pin(away_with(&env, env.context(), &[], true, vec![summary()])).await;
    assert_eq!(ran.exit, Exit::Success);
    let asked = ran.asked.unwrap();
    assert_eq!((asked.conversation_id, asked.presence), (Some(conversation()), Presence::Away));
    assert_eq!(
        ran.stdout,
        "away: a question now waits 5 minutes, then its call does not run; your next input \
         here ends this\n"
    );
    assert_eq!(ran.stderr, "the newest conversation; efr away --conversation <id> sets another\n");

    // A named conversation needs no line about it, and one that was away says so.
    let env = TestEnv::new();
    let id = conversation().to_string();
    let ran =
        Box::pin(away_with(&env, env.context(), &["--conversation", &id], false, Vec::new())).await;
    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.stdout.starts_with("away already: "), "{}", ran.stdout);
    assert_eq!(ran.stderr, "");
}

#[tokio::test]
async fn without_a_conversation_there_is_none_to_be_away_from() {
    let env = TestEnv::new();
    let ran = Box::pin(away_with(&env, env.context(), &[], true, Vec::new())).await;
    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(ran.asked.is_none());
    assert_eq!(ran.stderr, "efr: there is no conversation yet, so there is none to be away from\n");
}

#[test]
fn the_line_names_the_wait_in_minutes_when_it_can() {
    assert!(away_line(true, Duration::from_secs(60)).contains("waits 1 minute,"));
    assert!(away_line(true, Duration::from_secs(600)).contains("waits 10 minutes,"));
    assert!(away_line(true, Duration::from_secs(90)).contains("waits 1m 30s,"));
}
