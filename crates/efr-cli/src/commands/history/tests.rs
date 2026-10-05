use efr_protocol::{
    ApprovalDecision, CommandId, ConversationHistoryResult, ConversationStatus,
    ConversationSummary, ConversationsListResult, EffectiveSettings, ErrorBody, ErrorCode, Event,
    EventEnvelope, Method, Mode, Origin, OverriddenSettings, PageCursor, Scope, Seq, TurnSettings,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::transcript;
use crate::error::Exit;
use crate::run;
use crate::testing::{
    CONVERSATION, TestEnv, call, capture, command, conversation, envelope, now, readable, turn,
};

fn summary(id: &str) -> ConversationSummary {
    ConversationSummary {
        id: id.parse().unwrap(),
        title: Some("check nginx".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: None,
    }
}

fn events() -> Vec<EventEnvelope> {
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    vec![
        envelope(
            1,
            Event::PromptQueued {
                turn_id: turn(),
                command_id,
                text: "is nginx\nhealthy?".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
            },
        ),
        envelope(
            2,
            Event::ToolCallStarted {
                turn_id: turn(),
                call_id: call(),
                tool: "shell".to_owned(),
                input: json!({ "command": "systemctl status nginx" }),
            },
        ),
        envelope(
            3,
            Event::ApprovalRequested {
                turn_id: turn(),
                call_id: call(),
                summary: "run systemctl restart nginx".to_owned(),
                diff_preview: None,
            },
        ),
        envelope(
            4,
            Event::ApprovalResolved {
                turn_id: turn(),
                call_id: call(),
                decision: ApprovalDecision::Allow,
                origin: Origin::Phone,
            },
        ),
        envelope(
            5,
            Event::AssistantMessageUpdated {
                turn_id: turn(),
                index: 0,
                offset: 0,
                delta: "It is".to_owned(),
            },
        ),
        envelope(
            6,
            Event::AssistantMessageCompleted {
                turn_id: turn(),
                index: 0,
                text: "It is **running** again.".to_owned(),
            },
        ),
        envelope(
            7,
            Event::AssistantMessageUpdated {
                turn_id: turn(),
                index: 1,
                offset: 0,
                delta: "Cut off".to_owned(),
            },
        ),
        envelope(
            8,
            Event::AssistantMessageUpdated {
                turn_id: turn(),
                index: 1,
                offset: 7,
                delta: " mid".to_owned(),
            },
        ),
        envelope(9, Event::TurnInterrupted { turn_id: turn() }),
    ]
}

#[test]
fn a_transcript_on_a_terminal() {
    let page = ConversationHistoryResult { events: events(), next_cursor: None };
    insta::assert_snapshot!(readable(&transcript(conversation(), &page, &RenderOptions::new(60))));
}

#[test]
fn a_raw_transcript_with_an_earlier_page() {
    let page =
        ConversationHistoryResult { events: events(), next_cursor: Some(PageCursor::new("c-1")) };
    let options = RenderOptions::new(60).with_terminal(false);
    insta::assert_snapshot!(transcript(conversation(), &page, &options));
}

#[test]
fn a_failed_turn_shows_its_message_safely() {
    let page = ConversationHistoryResult {
        events: vec![envelope(
            1,
            Event::TurnFailed {
                turn_id: turn(),
                error: ErrorBody::new(ErrorCode::Internal, "boom\u{1b}[2J"),
            },
        )],
        next_cursor: None,
    };
    let text = transcript(conversation(), &page, &RenderOptions::new(60).with_terminal(false));
    assert!(text.ends_with("failed: boom\u{241b}[2J\n"), "{text}");
}

#[tokio::test]
async fn history_without_a_conversation_lists_them() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::ConversationsList(params) = method else { panic!("expected a list") };
        assert_eq!(params.limit, Some(5));
        let list = ConversationsListResult {
            conversations: vec![summary(CONVERSATION)],
            next_cursor: None,
        };
        conn.reply(id, &list).await;
        conn.until_closed().await;
    };
    let line = command(&["history", "--limit", "5"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let expected = format!("{CONVERSATION}  {:<17}  {:>11}  check nginx\n", "idle", "just now");
    assert_eq!(captured.stdout(), expected);
}

#[tokio::test]
async fn history_of_a_whole_id_asks_for_its_events() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        let Method::ConversationHistory(params) = method else { panic!("expected history") };
        assert_eq!(params.conversation_id, conversation());
        assert_eq!(params.cursor, Some(PageCursor::new("c-1")));
        conn.reply(id, &ConversationHistoryResult::default()).await;
        conn.until_closed().await;
    };
    let line = command(&["history", CONVERSATION, "--cursor", "c-1"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    assert_eq!(captured.stdout(), format!("conversation {CONVERSATION}\n"));
}

#[tokio::test]
async fn history_matches_the_start_of_an_id() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let list = ConversationsListResult {
            conversations: vec![
                summary("019a9b1c-3d00-7a10-8b20-000000000001"),
                summary("029a9b1c-3d00-7a10-8b20-000000000002"),
            ],
            next_cursor: None,
        };
        conn.reply(id, &list).await;
        let (id, method) = conn.request().await;
        assert!(
            matches!(method, Method::ConversationHistory(ref p) if p.conversation_id == conversation())
        );
        conn.reply(id, &ConversationHistoryResult::default()).await;
        conn.until_closed().await;
    };
    let line = command(&["history", "019A9B"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
}

#[tokio::test]
async fn an_ambiguous_start_of_an_id_is_a_usage_error() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let list = ConversationsListResult {
            conversations: vec![
                summary("019a9b1c-3d00-7a10-8b20-000000000001"),
                summary("019a9b1c-3d00-7a10-8b20-000000000002"),
            ],
            next_cursor: None,
        };
        conn.reply(id, &list).await;
        conn.until_closed().await;
    };
    let line = command(&["history", "019a9b1c"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Usage);
    assert_eq!(
        captured.stderr(),
        "efr: \"019a9b1c\" matches 2 conversations; type more of the id\n"
    );
}

#[tokio::test]
async fn a_start_of_an_id_that_matches_nothing_fails() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.reply(id, &ConversationsListResult::default()).await;
        conn.until_closed().await;
    };
    let line = command(&["history", "beef"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(captured.stderr(), "efr: no conversation matches \"beef\"\n");
}

#[tokio::test]
async fn a_very_short_start_of_an_id_matches_nothing_without_asking() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, _captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.until_closed().await, []);
    };
    let line = command(&["history", "01"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
}

#[test]
fn each_turn_shows_its_mode_model_and_effort_after_its_prompt() {
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let prompt = |seq: u64, text: &str| {
        envelope(
            seq,
            Event::PromptQueued {
                turn_id: turn(),
                command_id,
                text: text.to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
            },
        )
    };
    let started = |seq: u64, settings: Option<EffectiveSettings>| {
        envelope(
            seq,
            Event::TurnStarted {
                turn_id: turn(),
                cwd: "/home/user".into(),
                scope: Scope::Machine,
                settings,
            },
        )
    };
    let auto = EffectiveSettings {
        mode: Mode::Auto,
        model: "gpt-5.4".to_owned(),
        effort: Some("high".to_owned()),
        overridden: OverriddenSettings { mode: true, ..OverriddenSettings::default() },
    };
    let cautious = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: None,
        overridden: OverriddenSettings::default(),
    };
    let page = ConversationHistoryResult {
        events: vec![
            prompt(1, "an old turn"),
            started(2, None),
            prompt(3, "build it"),
            started(4, Some(auto)),
            prompt(5, "and check"),
            started(6, Some(cautious)),
        ],
        next_cursor: None,
    };

    let text = transcript(conversation(), &page, &RenderOptions::new(60).with_terminal(false));

    insta::assert_snapshot!(text);
}
