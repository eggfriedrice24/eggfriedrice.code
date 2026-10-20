use efr_protocol::{
    CacheSummary, ClientFrame, CompactionSummary, ContextPart, ContextPartKind,
    ConversationContext, ConversationContextResult, ConversationStatus, ConversationSummary,
    ConversationsListResult, Method, Seq,
};
use pretty_assertions::assert_eq;
use serde_json::Value;

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

fn result() -> ConversationContextResult {
    ConversationContextResult {
        model: "gpt-5.5".to_owned(),
        window: 272_000,
        window_known: true,
        trigger: 206_720,
        hard_cap: 258_400,
        auto: true,
        counted: None,
        estimate: 3_000,
        running: false,
        parts: vec![
            ContextPart { kind: ContextPartKind::System, tokens: 2_000, count: 1 },
            ContextPart { kind: ContextPartKind::Prompts, tokens: 1_000, count: 1 },
        ],
        largest: Vec::new(),
        compactions: CompactionSummary::default(),
        cache: CacheSummary::default(),
    }
}

/// What a run of `efr context <args>` did against a daemon that lists `listed`.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    asked: Option<ConversationContext>,
}

async fn context_with(
    env: &TestEnv,
    ctx: Context,
    args: &[&str],
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
                Method::ConversationContext(params) => {
                    asked = Some(params);
                    conn.reply(id, &result()).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let mut line = vec!["context"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: captured.stdout(), stderr: captured.stderr(), asked }
}

#[tokio::test]
async fn efr_context_shows_the_newest_conversation_and_says_which() {
    let env = TestEnv::new();
    let ran = Box::pin(context_with(&env, env.context(), &[], vec![summary()])).await;

    assert_eq!(ran.exit, Exit::Success);
    let asked = ran.asked.unwrap();
    assert_eq!((asked.conversation_id, asked.model), (conversation(), None));
    assert!(ran.stdout.starts_with("model        gpt-5.5, window 272k"), "{}", ran.stdout);
    assert!(ran.stdout.contains("\n  prompts         1.0k  1 prompt\n"), "{}", ran.stdout);
    assert_eq!(ran.stderr, "the newest conversation; efr context <id> shows another\n");
}

#[tokio::test]
async fn context_json() {
    let env = TestEnv::new();
    let id = conversation().to_string();
    let args = [id.as_str(), "--json", "--model", "gpt-5.4"];
    let ran = Box::pin(context_with(&env, env.context(), &args, Vec::new())).await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.asked.unwrap().model.as_deref(), Some("gpt-5.4"));
    assert_eq!(ran.stderr, "", "a named conversation needs no line about it");
    let printed: Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(printed, serde_json::to_value(result()).unwrap(), "the daemon's answer as it came");
    assert!(ran.stdout.ends_with("}\n"));
}

#[tokio::test]
async fn without_a_conversation_it_says_so() {
    let env = TestEnv::new();
    let ran = Box::pin(context_with(&env, env.context(), &[], Vec::new())).await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.asked, None);
    assert_eq!(ran.stdout, "no conversations yet\n");
}
