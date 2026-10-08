use std::path::{Path, PathBuf};

use efr_protocol::{
    ApprovalDecision, BlockReason, Blocked, CommandId, ConversationHistoryResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, EffectiveSettings, ErrorBody,
    ErrorCode, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitSource, JudgeKind, Launch,
    Method, Mode, ModeFallback, Origin, OverriddenSettings, PageCursor, QuestionId, ReportedFile,
    SandboxSummary, Scope, Seq, SurfaceChange, TurnSettings, Verdict,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Shown, transcript};
use crate::error::Exit;
use crate::run;
use crate::testing::{
    CONVERSATION, FAILED_UNITS, FROM_SRC, TestEnv, call, capture, command, conversation, envelope,
    exit_info, exit_record, now, program_fact, readable, turn,
};

/// The transcript of `page`, not verbose.
fn show(page: &ConversationHistoryResult, options: &RenderOptions) -> String {
    transcript(conversation(), page, &Shown { options, verbose: false, home: None })
}

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
                steers: Vec::new(),
            },
        ),
        envelope(
            2,
            Event::ToolCallStarted {
                turn_id: turn(),
                call_id: call(),
                tool: "shell".to_owned(),
                input: json!({ "command": "systemctl status nginx" }),
                manual_input: true,
                launch: None,
                freeform: false,
            },
        ),
        envelope(
            3,
            Event::ApprovalRequested {
                turn_id: turn(),
                call_id: call(),
                summary: "run systemctl restart nginx".to_owned(),
                diff_preview: None,
                interactive: false,
                exit: None,
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
        envelope(9, Event::TurnInterrupted { turn_id: turn(), usage: None, context: None }),
    ]
}

#[test]
fn a_transcript_on_a_terminal() {
    let page = ConversationHistoryResult { events: events(), next_cursor: None };
    insta::assert_snapshot!(readable(&show(&page, &RenderOptions::new(60))));
}

#[test]
fn a_raw_transcript_with_an_earlier_page() {
    let page =
        ConversationHistoryResult { events: events(), next_cursor: Some(PageCursor::new("c-1")) };
    let options = RenderOptions::new(60).with_terminal(false);
    insta::assert_snapshot!(show(&page, &options));
}

#[test]
fn a_failed_turn_shows_its_message_safely() {
    let page = ConversationHistoryResult {
        events: vec![envelope(
            1,
            Event::TurnFailed {
                turn_id: turn(),
                error: ErrorBody::new(ErrorCode::Internal, "boom\u{1b}[2J"),
                usage: None,
                context: None,
            },
        )],
        next_cursor: None,
    };
    let text = show(&page, &RenderOptions::new(60).with_terminal(false));
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
                steers: Vec::new(),
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
        fallback: None,
    };
    let cautious = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: None,
        overridden: OverriddenSettings::default(),
        fallback: None,
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

    let text = show(&page, &RenderOptions::new(60).with_terminal(false));

    insta::assert_snapshot!(text);
}

/// A turn in `auto` that fell back, then the sandbox's events of one call.
fn sandbox_events() -> Vec<EventEnvelope> {
    let question: QuestionId = "019a9b1c-3d00-7a10-8b20-0000000000d1".parse().unwrap();
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: None,
        overridden: OverriddenSettings::default(),
        fallback: Some(ModeFallback { asked: Mode::Auto, reason: "Landlock ABI 6".to_owned() }),
    };
    let facts = ExitFacts {
        programs: vec![program_fact("sudo", "/usr/bin/sudo")],
        refusals_in_a_row: 1,
        ..ExitFacts::default()
    };
    let record = exit_record("sudo pacman -Syu", facts);
    let change = |path: &str| SurfaceChange {
        path: PathBuf::from(path),
        rule: "commondir_in_main_git_dir".to_owned(),
        key: Some("core.fsmonitor".to_owned()),
        quarantined: true,
    };
    let events = vec![
        Event::TurnStarted {
            turn_id: turn(),
            cwd: "/home/user/project".into(),
            scope: Scope::Machine,
            settings: Some(settings),
        },
        Event::ToolCallStarted {
            turn_id: turn(),
            call_id: call(),
            tool: "shell".to_owned(),
            input: json!({ "command": "cargo add serde" }),
            manual_input: true,
            launch: Some(Launch::contained()),
            freeform: false,
        },
        Event::ToolCallCompleted {
            turn_id: turn(),
            call_id: call(),
            output: String::new(),
            truncated: false,
            is_error: false,
            exit_code: Some(101),
            sandbox: Some(SandboxSummary {
                confined: true,
                blocked: vec![Blocked {
                    host: "index.crates.io".to_owned(),
                    port: 443,
                    reason: BlockReason::NotAllowed,
                }],
                ..SandboxSummary::default()
            }),
            refusal: None,
            changes: None,
            diff: None,
        },
        Event::ExitRequested {
            turn_id: turn(),
            call_id: call(),
            kinds: vec![ExitKind::Privilege],
            grants: Vec::new(),
            source: ExitSource::Predicted,
            record: Box::new(record),
        },
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "run `sudo pacman -Syu`".to_owned(),
            diff_preview: None,
            interactive: true,
            exit: Some(ExitInfo {
                user_only: true,
                ..exit_info(&[ExitKind::Privilege], Launch::Unsandboxed)
            }),
        },
        Event::ExitJudged {
            turn_id: turn(),
            call_id: call(),
            judge: JudgeKind::User,
            verdict: Verdict::Allow,
            model: None,
            latency_ms: None,
            risk: None,
            user_authorization: None,
            category: None,
            rationale: None,
            record_sha256: None,
            cached: false,
        },
        Event::SandboxSurfaceChanged {
            turn_id: turn(),
            call_id: call(),
            changes: vec![change("/home/user/project/.git/commondir")],
            quarantined: true,
        },
        Event::SurfaceQuestionRequested {
            turn_id: turn(),
            call_id: call(),
            question_id: question,
            changes: vec![change("/home/user/project/.git/commondir")],
        },
        Event::SurfaceQuestionAnswered {
            turn_id: turn(),
            question_id: question,
            keep: false,
            origin: None,
        },
        Event::TurnSurfaceReport {
            turn_id: turn(),
            files: vec![ReportedFile {
                path: PathBuf::from(".cargo/config.toml"),
                detail: Some("build.rustc-wrapper".to_owned()),
            }],
        },
        Event::SandboxUnavailable { reason: "bubblewrap is not installed".to_owned() },
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn the_sandbox_shows_its_notes_and_verbose_adds_each_exit_record() {
    let page = ConversationHistoryResult { events: sandbox_events(), next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain =
        transcript(conversation(), &page, &Shown { options: &options, verbose: false, home });
    assert!(!plain.contains("exit requested"), "{plain}");
    let verbose =
        transcript(conversation(), &page, &Shown { options: &options, verbose: true, home });
    insta::assert_snapshot!(format!("{plain}---\n{verbose}"));
}

#[test]
fn a_withdrawn_prompt_says_that_it_never_ran() {
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let events = vec![
        Event::PromptQueued {
            turn_id: turn(),
            command_id,
            text: "later".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
        Event::PromptWithdrawn { turn_id: turn(), origin: Origin::Shell },
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);
    let shown =
        transcript(conversation(), &page, &Shown { options: &options, verbose: false, home: None });
    assert!(shown.contains("later"), "{shown}");
    assert!(shown.contains("withdrawn before it ran"), "{shown}");
}

#[test]
fn each_turn_shows_together_in_the_order_the_turns_started() {
    let id = |n: u8| -> efr_protocol::TurnId {
        format!("019a9b1c-3d00-7a10-8b20-0000000001{n:02x}").parse().unwrap()
    };
    let (first, docs, changelog, resent) = (id(1), id(2), id(3), id(4));
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let prompt = |turn_id, text: &str, steers: Vec<Seq>| Event::PromptQueued {
        turn_id,
        command_id,
        text: text.to_owned(),
        origin: Origin::Shell,
        context: None,
        settings: TurnSettings::default(),
        steers,
    };
    let started = |turn_id| Event::TurnStarted {
        turn_id,
        cwd: "/home/user".into(),
        scope: Scope::Machine,
        settings: None,
    };
    let answer = |turn_id, text: &str| Event::AssistantMessageCompleted {
        turn_id,
        index: 0,
        text: text.to_owned(),
    };
    let done =
        |turn_id| Event::TurnCompleted { turn_id, usage: None, changes: None, context: None };
    // While the first turn runs, the user queues two prompts and takes the second
    // back, then steers and presses Esc: the steer runs next as a new prompt.
    let events = vec![
        prompt(first, "fix the bug", Vec::new()),
        started(first),
        Event::ToolCallStarted {
            turn_id: first,
            call_id: call(),
            tool: "shell".to_owned(),
            input: json!({ "command": "cargo test" }),
            manual_input: true,
            launch: None,
            freeform: false,
        },
        prompt(docs, "then update the docs", Vec::new()),
        prompt(changelog, "and the changelog", Vec::new()),
        Event::PromptWithdrawn { turn_id: changelog, origin: Origin::Shell },
        Event::TurnSteered { turn_id: first, text: "look at the logs first".to_owned() },
        Event::TurnInterruptRequested { turn_id: first, origin: Origin::Shell },
        prompt(resent, "look at the logs first", vec![Seq::new(7)]),
        Event::ToolCallCompleted {
            turn_id: first,
            call_id: call(),
            output: String::new(),
            truncated: false,
            is_error: true,
            exit_code: Some(130),
            sandbox: None,
            refusal: None,
            changes: None,
            diff: None,
        },
        Event::TurnInterrupted { turn_id: first, usage: None, context: None },
        started(resent),
        answer(resent, "The logs show a timeout."),
        done(resent),
        started(docs),
        answer(docs, "Docs updated."),
        done(docs),
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };

    let text = show(&page, &RenderOptions::new(60).with_terminal(false));

    let at = |line: &str| text.find(line).unwrap_or_else(|| panic!("no {line:?} in {text}"));
    let order = [
        "> fix the bug",
        "$ cargo test",
        "steered: look at the logs first",
        "interrupted",
        "> look at the logs first",
        "The logs show a timeout.",
        "> then update the docs",
        "Docs updated.",
        "> and the changelog",
        "withdrawn before it ran",
    ];
    for pair in order.windows(2) {
        assert!(at(pair[0]) < at(pair[1]), "{:?} before {:?}:\n{text}", pair[0], pair[1]);
    }
    insta::assert_snapshot!(text);
}

#[test]
fn a_turn_that_started_before_the_page_comes_first() {
    let next: efr_protocol::TurnId = "019a9b1c-3d00-7a10-8b20-000000000102".parse().unwrap();
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let page = vec![
        envelope(
            20,
            Event::PromptQueued {
                turn_id: next,
                command_id,
                text: "next".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
                steers: Vec::new(),
            },
        ),
        envelope(21, Event::TurnInterrupted { turn_id: turn(), usage: None, context: None }),
        envelope(
            22,
            Event::TurnStarted {
                turn_id: next,
                cwd: "/home/user".into(),
                scope: Scope::Machine,
                settings: None,
            },
        ),
    ];
    let seqs: Vec<Vec<u64>> = super::in_turn_order(&page)
        .iter()
        .map(|group| group.iter().map(|envelope| envelope.seq.get()).collect())
        .collect();
    assert_eq!(seqs, [vec![21], vec![20, 22]]);
}

#[test]
fn a_steer_taken_back_says_that_the_model_did_not_read_it() {
    let events = vec![
        Event::TurnSteered { turn_id: turn(), text: "use tabs".to_owned() },
        Event::TurnInterruptRequested { turn_id: turn(), origin: Origin::Shell },
        Event::SteeringWithdrawn {
            turn_id: turn(),
            steers: vec![Seq::new(1)],
            origin: Origin::Shell,
        },
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);
    let shown =
        transcript(conversation(), &page, &Shown { options: &options, verbose: false, home: None });
    assert!(shown.contains("took back a steer that the model did not read"), "{shown}");
}

#[test]
fn a_command_of_several_lines_never_shows_as_one_line() {
    let other: efr_protocol::CallId = "019a9b1c-3d00-7a10-8b20-0000000000c2".parse().unwrap();
    let started = |call_id, command: &str| Event::ToolCallStarted {
        turn_id: turn(),
        call_id,
        tool: "shell".to_owned(),
        input: json!({ "command": command }),
        manual_input: true,
        launch: None,
        freeform: false,
    };
    let grants = vec![efr_protocol::Grant::Bus { bus: efr_protocol::BusKind::System }];
    let info = exit_info(&[ExitKind::Bus], Launch::Contained { grants: grants.clone() });
    let events = vec![
        started(call(), FAILED_UNITS),
        Event::ExitRequested {
            turn_id: turn(),
            call_id: call(),
            kinds: vec![ExitKind::Bus],
            grants,
            source: ExitSource::Predicted,
            record: Box::new(exit_record(FAILED_UNITS, ExitFacts::default())),
        },
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: format!("shell: run {FAILED_UNITS:?}"),
            diff_preview: None,
            interactive: false,
            exit: Some(info),
        },
        started(other, FROM_SRC),
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: other,
            summary: format!("shell: run {FROM_SRC:?}"),
            diff_preview: None,
            interactive: false,
            exit: None,
        },
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain =
        transcript(conversation(), &page, &Shown { options: &options, verbose: false, home });
    let verbose =
        transcript(conversation(), &page, &Shown { options: &options, verbose: true, home });
    insta::assert_snapshot!(format!("{plain}---\n{verbose}"));
}

#[test]
fn a_patch_shows_its_files_and_never_its_text() {
    let other: efr_protocol::CallId = "019a9b1c-3d00-7a10-8b20-0000000000c2".parse().unwrap();
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@ fn main\n-    old();\n+    new();\n*** Add File: notes.md\n+# Notes\n*** Delete File: old.rs\n*** Update File: src/expr.rs\n*** Move to: src/expression.rs\n*** End Patch\n";
    let started = |call_id, input, freeform| Event::ToolCallStarted {
        turn_id: turn(),
        call_id,
        tool: "apply_patch".to_owned(),
        input,
        manual_input: false,
        launch: None,
        freeform,
    };
    let completed = |call_id, is_error| Event::ToolCallCompleted {
        turn_id: turn(),
        call_id,
        output: "Success.".to_owned(),
        truncated: false,
        is_error,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
    };
    let events = vec![
        // The freeform form: the input is the text.
        started(call(), json!(patch), true),
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "apply_patch: edit src/a.rs, notes.md; delete old.rs; move src/expr.rs"
                .to_owned(),
            diff_preview: Some("delete old.rs\n--- a/old.rs\n+++ /dev/null\n".to_owned()),
            interactive: false,
            exit: None,
        },
        Event::ApprovalResolved {
            turn_id: turn(),
            call_id: call(),
            decision: ApprovalDecision::Allow,
            origin: Origin::Shell,
        },
        completed(call(), false),
        // The function form: the text is the member `input`.
        started(
            other,
            json!({ "input": "*** Begin Patch\n*** Update File: src/b.rs\n+x\n*** End Patch\n" }),
            false,
        ),
        completed(other, true),
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let terminal = show(&page, &RenderOptions::new(60));
    let piped = show(&page, &RenderOptions::new(60).with_terminal(false));
    assert!(!piped.contains("Begin Patch"), "{piped}");
    assert!(
        piped.contains(
            "apply_patch src/a.rs +1 \u{2212}1, new notes.md +1, delete old.rs, move src/expr.rs \u{2192} src/expression.rs"
        ),
        "{piped}"
    );
    insta::assert_snapshot!(format!("{}---\n{piped}", readable(&terminal)));
}

/// What `efr history --verbose` without a conversation writes when the terminal is
/// `tty` and the daemon lists `listed`; the history it asks for gets the sandbox's
/// events.
async fn verbose_without_a_conversation(
    tty: Option<&str>,
    listed: Vec<ConversationSummary>,
) -> (Exit, String, Option<efr_protocol::ConversationId>) {
    let env = TestEnv::new();
    let daemon = env.listen();
    let mut ctx = env.context();
    ctx.tty = tty.map(str::to_owned);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut shown = None;
        loop {
            let Some(frame) = conn.recv().await else { break };
            let efr_protocol::ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(params) => {
                    let mut conversations = listed.clone();
                    if let Some(limit) = params.limit {
                        conversations.truncate(limit as usize);
                    }
                    let list = ConversationsListResult { conversations, next_cursor: None };
                    conn.reply(id, &list).await;
                }
                Method::ConversationHistory(params) => {
                    shown = Some(params.conversation_id);
                    let page =
                        ConversationHistoryResult { events: sandbox_events(), next_cursor: None };
                    conn.reply(id, &page).await;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        shown
    };
    let line = command(&["history", "--verbose"]);
    let (exit, shown) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    (exit, captured.stdout(), shown)
}

#[tokio::test]
async fn history_verbose_without_a_conversation_shows_the_newest_of_this_terminal() {
    let newest = "019a9b1c-3d00-7a10-8b20-0000000000a1";
    let mut mine = summary(CONVERSATION);
    mine.tty = Some("/dev/pts/3".to_owned());
    let (exit, stdout, shown) =
        verbose_without_a_conversation(Some("/dev/pts/3"), vec![summary(newest), mine]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(shown, Some(conversation()));
    assert!(
        stdout.starts_with(&format!(
            "the newest conversation of this terminal; efr history lists them all\n\
             conversation {CONVERSATION}\n"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("exit requested: privilege (predicted from the line)"), "{stdout}");
}

#[tokio::test]
async fn history_verbose_without_a_conversation_falls_back_to_the_newest() {
    let newest = "019a9b1c-3d00-7a10-8b20-0000000000a1";
    let (exit, stdout, shown) =
        verbose_without_a_conversation(Some("/dev/pts/9"), vec![summary(newest)]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(shown, Some(newest.parse().unwrap()));
    assert!(
        stdout.starts_with(&format!(
            "the newest conversation; efr history lists them all\nconversation {newest}\n"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("exit requested:"), "{stdout}");

    let (_, stdout, shown) = verbose_without_a_conversation(None, vec![summary(newest)]).await;
    assert_eq!(shown, Some(newest.parse().unwrap()));
    assert!(stdout.starts_with("the newest conversation;"), "{stdout}");

    let (exit, stdout, shown) = verbose_without_a_conversation(None, Vec::new()).await;
    assert_eq!((exit, stdout.as_str(), shown), (Exit::Success, "no conversations yet\n", None));
}

#[tokio::test]
async fn history_verbose_is_a_flag() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let page = ConversationHistoryResult { events: sandbox_events(), next_cursor: None };
        conn.reply(id, &page).await;
        conn.until_closed().await;
    };
    let line = command(&["history", CONVERSATION, "--verbose"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success);
    let stdout = captured.stdout();
    assert!(stdout.contains("exit requested: privilege (predicted from the line)"), "{stdout}");
    assert!(stdout.contains("  program sudo /usr/bin/sudo\n"), "{stdout}");
}

#[test]
fn a_compaction_marks_its_place_and_verbose_shows_its_summary() {
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c2".parse().unwrap();
    let compaction = efr_protocol::Compaction {
        compaction_id: "019a9b1c-3d00-7a10-8b20-0000000000c3".parse().unwrap(),
        turn_id: None,
        trigger: efr_protocol::CompactionTrigger::Manual,
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
        summary: Some("## Task and state\nFree space on /var.".to_owned()),
        usage: None,
    };
    let events = vec![
        Event::PromptQueued {
            turn_id: turn(),
            command_id,
            text: "free space".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
        Event::ConversationCompacted(compaction),
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);

    let plain =
        transcript(conversation(), &page, &Shown { options: &options, verbose: false, home: None });
    let verbose =
        transcript(conversation(), &page, &Shown { options: &options, verbose: true, home: None });

    let line = "context compacted (efr compact): 140k -> 19.0k tokens, kept 1 turn, summary 10";
    assert!(plain.contains(line), "{plain}");
    assert!(!plain.contains("Free space on /var."), "{plain}");
    assert!(verbose.contains(line), "{verbose}");
    assert!(verbose.contains("  ## Task and state\n  Free space on /var.\n"), "{verbose}");
}
