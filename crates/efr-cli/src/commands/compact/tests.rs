use efr_protocol::{
    ClientFrame, Compaction, CompactionTrigger, ConversationCompact, ConversationCompactResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, ErrorBody, ErrorCode, Method,
    Seq,
};
use pretty_assertions::assert_eq;

use crate::error::Exit;
use crate::run;
use crate::testing::{TestEnv, capture, command, conversation, now, turn};

const TTY: &str = "/dev/pts/7";

fn listed() -> ConversationSummary {
    ConversationSummary {
        id: conversation(),
        title: Some("free space".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: Some(TTY.to_owned()),
    }
}

fn result() -> ConversationCompactResult {
    ConversationCompactResult {
        seq: Seq::new(10),
        compaction: Compaction {
            compaction_id: conversation().to_string().parse().expect("an id"),
            turn_id: None,
            trigger: CompactionTrigger::Manual,
            focus: Some("the disk".to_owned()),
            model: "gpt-5.5".to_owned(),
            window: 272_000,
            limit: 206_720,
            tokens_before: 140_000,
            tokens_after: 19_000,
            through_turn: turn(),
            through_message: None,
            kept_turns: 1,
            pruned_outputs: 0,
            pruned_tokens: 0,
            summary: Some("## Task and state\nFree space.".to_owned()),
            usage: Some(efr_protocol::Usage::new(139_000, 2_100)),
        },
    }
}

/// Runs `efr compact <args>` from the terminal `tty` against a daemon that lists the
/// one conversation and answers `conversation.compact` with `answer`; returns the exit,
/// stdout, stderr and the params it got.
async fn compact_with(
    args: &[&str],
    tty: Option<&str>,
    answer: Result<ConversationCompactResult, ErrorBody>,
) -> (Exit, String, String, Option<ConversationCompact>) {
    let env = TestEnv::new();
    let daemon = env.listen();
    let mut ctx = env.context();
    ctx.tty = tty.map(str::to_owned);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut asked = None;
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: vec![listed()],
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
    (exit, captured.stdout(), captured.stderr(), asked)
}

#[tokio::test]
async fn efr_compact_compacts_the_terminals_conversation_with_the_focus() {
    let (exit, stdout, stderr, asked) =
        compact_with(&["the", "disk"], Some(TTY), Ok(result())).await;

    assert_eq!(exit, Exit::Success, "{stderr}");
    let asked = asked.expect("conversation.compact");
    assert_eq!(asked.conversation_id, conversation());
    assert_eq!(asked.focus.as_deref(), Some("the disk"));
    assert_eq!(
        stdout,
        "context compacted (efr compact): 140k -> 19.0k tokens, kept 1 turn, summary 2.1k\n"
    );
}

#[tokio::test]
async fn without_a_terminal_or_a_conversation_efr_compact_is_a_usage_error() {
    let (exit, _, stderr, asked) = compact_with(&[], None, Ok(result())).await;

    assert_eq!(exit, Exit::Usage);
    assert_eq!(asked, None);
    assert!(stderr.contains("efr compact needs --conversation"), "{stderr}");
}

#[tokio::test]
async fn a_refusal_of_the_daemon_is_the_error() {
    let busy = ErrorBody::new(ErrorCode::Conflict, "the conversation is busy with a turn");
    let id = conversation().to_string();

    let (exit, stdout, stderr, asked) =
        compact_with(&["--conversation", &id], None, Err(busy)).await;

    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(asked.map(|asked| asked.focus), Some(None));
    assert_eq!(stdout, "");
    assert!(stderr.contains("busy with a turn"), "{stderr}");
}
