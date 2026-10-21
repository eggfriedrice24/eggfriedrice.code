use std::path::{Path, PathBuf};

use efr_config::DiffGroups;
use efr_protocol::{
    ApprovalDecision, BlockReason, Blocked, ChainStep, CommandId, CompactionTrigger,
    ConversationHistoryResult, ConversationStatus, ConversationSummary, ConversationsListResult,
    EffectiveSettings, ErrorBody, ErrorCode, Event, EventEnvelope, ExitFacts, ExitInfo, ExitKind,
    ExitSource, JudgeKind, Launch, Method, Mode, ModeFallback, Origin, OverriddenSettings,
    PageCursor, PauseReason, Presence, PresenceSource, QuestionId, ReportedFile, Risk,
    SandboxSummary, Scope, Seq, SurfaceChange, TurnSettings, Verdict,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Shown, transcript};
use crate::error::Exit;
use crate::run;
use crate::testing::{
    CONVERSATION, FAILED_UNITS, FROM_SRC, TestEnv, call, capture, command, compaction,
    conversation, envelope, exit_info, exit_record, now, program_fact, readable, turn,
};

/// The transcript of `page`, not verbose.
fn show(page: &ConversationHistoryResult, options: &RenderOptions) -> String {
    transcript(
        conversation(),
        page,
        &Shown { options, verbose: false, home: None, groups: &DiffGroups::default() },
    )
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
                expires_in_secs: None,
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
        envelope(
            9,
            Event::TurnInterrupted {
                turn_id: turn(),
                usage: None,
                context: None,
                changes: None,
                undo: None,
            },
        ),
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
                changes: None,
                undo: None,
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
                    count: 1,
                }],
                ..SandboxSummary::default()
            }),
            refusal: None,
            changes: None,
            diff: None,
            left_running: false,
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
            expires_in_secs: None,
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
            chain_step: None,
            skipped: Vec::new(),
            reasked: false,
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
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    assert!(!plain.contains("exit requested"), "{plain}");
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
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
    let shown = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home: None, groups: &DiffGroups::default() },
    );
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
    let done = |turn_id| Event::TurnCompleted {
        turn_id,
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    };
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
            left_running: false,
        },
        Event::TurnInterrupted {
            turn_id: first,
            usage: None,
            context: None,
            changes: None,
            undo: None,
        },
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
        envelope(
            21,
            Event::TurnInterrupted {
                turn_id: turn(),
                usage: None,
                context: None,
                changes: None,
                undo: None,
            },
        ),
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
    let shown = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home: None, groups: &DiffGroups::default() },
    );
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
            expires_in_secs: None,
        },
        started(other, FROM_SRC),
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: other,
            summary: format!("shell: run {FROM_SRC:?}"),
            diff_preview: None,
            interactive: false,
            exit: None,
            expires_in_secs: None,
        },
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(100).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
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
        left_running: false,
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
            expires_in_secs: None,
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
            "apply_patch src/a.rs +1 -1, new notes.md +1, delete old.rs, move src/expr.rs \u{2192} src/expression.rs"
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
    verbose_from(tty, listed, false).await
}

/// As [`verbose_without_a_conversation`], from a daemon that answers the `tty` filter
/// of `conversations.list` when `filters`, as a daemon with routing by scope does: only
/// that terminal's rows, in the order of `listed`.
async fn verbose_from(
    tty: Option<&str>,
    listed: Vec<ConversationSummary>,
    filters: bool,
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
                    if filters && let Some(tty) = &params.tty {
                        conversations.retain(|summary| summary.tty.as_ref() == Some(tty));
                    }
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
        Box::pin(verbose_without_a_conversation(Some("/dev/pts/3"), vec![summary(newest), mine]))
            .await;
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
        Box::pin(verbose_without_a_conversation(Some("/dev/pts/9"), vec![summary(newest)])).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(shown, Some(newest.parse().unwrap()));
    assert!(
        stdout.starts_with(&format!(
            "the newest conversation; efr history lists them all\nconversation {newest}\n"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("exit requested:"), "{stdout}");

    let (_, stdout, shown) =
        Box::pin(verbose_without_a_conversation(None, vec![summary(newest)])).await;
    assert_eq!(shown, Some(newest.parse().unwrap()));
    assert!(stdout.starts_with("the newest conversation;"), "{stdout}");

    let (exit, stdout, shown) = Box::pin(verbose_without_a_conversation(None, Vec::new())).await;
    assert_eq!((exit, stdout.as_str(), shown), (Exit::Success, "no conversations yet\n", None));
}

#[tokio::test]
async fn pick_takes_the_current_slot_of_the_terminal() {
    let other = "019a9b1c-3d00-7a10-8b20-0000000000a1";
    let older_slot = "019a9b1c-3d00-7a10-8b20-0000000000a2";
    let mut current = summary(CONVERSATION);
    current.tty = Some("/dev/pts/3".to_owned());
    let mut slot = summary(older_slot);
    slot.tty = Some("/dev/pts/3".to_owned());
    // The newest conversation of all is another terminal's; this terminal's current
    // slot comes first in its own rows, though another slot was updated later.
    let listed = vec![summary(other), current, slot];
    let (exit, _, shown) = Box::pin(verbose_from(Some("/dev/pts/3"), listed, true)).await;
    assert_eq!((exit, shown), (Exit::Success, Some(conversation())));
}

#[tokio::test]
async fn pick_scans_rows_on_an_older_efrd() {
    let mut mine = summary(CONVERSATION);
    mine.tty = Some("/dev/pts/3".to_owned());
    let listed = vec![summary("019a9b1c-3d00-7a10-8b20-0000000000a1"), mine];
    let (exit, _, shown) = Box::pin(verbose_from(Some("/dev/pts/3"), listed, false)).await;
    assert_eq!((exit, shown), (Exit::Success, Some(conversation())));
}

#[test]
fn history_shows_picked_up_in_tty() {
    let route: efr_protocol::RouteScope = serde_json::from_value(
        json!({"kind": "project", "value": "0192f0c1-7a00-7000-8000-0000000a4401"}),
    )
    .unwrap();
    let mut events = events();
    events.insert(
        0,
        envelope(
            0,
            Event::TerminalAttached {
                tty: "/dev/pts/7".to_owned(),
                route,
                shell_pid: Some(4242),
                how: efr_protocol::AttachHow::PickedUp,
            },
        ),
    );
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(60).with_terminal(false);
    insta::assert_snapshot!(show(&page, &options));
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

/// A turn that compacted its context on its own, then a compaction that the user asked
/// for between turns, with a focus.
fn compaction_events() -> Vec<EventEnvelope> {
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let mut manual = compaction(CompactionTrigger::Manual, 19_000);
    manual.focus = Some("the failing test\u{1b}[2J".to_owned());
    manual.summary = Some(
        "## Task and state\nFix the failing test.\n\n## Decisions\nNone.\u{1b}[2J\n".to_owned(),
    );
    let events = [
        Event::PromptQueued {
            turn_id: turn(),
            command_id,
            text: "run the tests".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
        Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100)),
        Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "The tests pass.".to_owned(),
        },
        Event::ConversationCompacted(manual),
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn a_compaction_marks_its_place_and_verbose_adds_its_summary() {
    let page = ConversationHistoryResult { events: compaction_events(), next_cursor: None };
    let mut shown = Vec::new();
    for (name, options) in [
        ("terminal, 40 columns", RenderOptions::new(40)),
        ("pipe", RenderOptions::new(80).with_terminal(false)),
    ] {
        for verbose in [false, true] {
            let shown_as =
                Shown { options: &options, verbose, home: None, groups: &DiffGroups::default() };
            let text = transcript(conversation(), &page, &shown_as);
            shown.push(format!("=== {name}, verbose {verbose}\n{}", readable(&text)));
        }
    }
    insta::assert_snapshot!(shown.join("\n"));
}

/// A turn of `auto` in phase A: the reviewer allows a write and refuses an upload,
/// pauses, a question expires while the user is away, efrd frees space in a cache, and
/// a floor file edit leaves a backup.
fn reviewer_events() -> Vec<EventEnvelope> {
    let second: efr_protocol::CallId = "019a9b1c-3d00-7a10-8b20-0000000000b2".parse().unwrap();
    let write = efr_protocol::Grant::Write { path: PathBuf::from("/home/user/.zshrc") };
    let judged = |call_id, judge, verdict, rationale: Option<&str>| Event::ExitJudged {
        turn_id: turn(),
        call_id,
        judge,
        verdict,
        model: Some("gpt-6-luna".to_owned()),
        latency_ms: Some(1_830),
        risk: Some(Risk::Low),
        user_authorization: None,
        category: Some("file edit".to_owned()),
        rationale: rationale.map(str::to_owned),
        record_sha256: None,
        cached: false,
        chain_step: Some(ChainStep::ClassifierModel),
        skipped: vec!["claude-haiku-5-5: missing".to_owned()],
        reasked: false,
    };
    let events = vec![
        Event::ExitRequested {
            turn_id: turn(),
            call_id: call(),
            kinds: vec![ExitKind::Persistence],
            grants: vec![write],
            source: ExitSource::Predicted,
            record: Box::new(exit_record(
                "echo 'alias k=kubectl' >> ~/.zshrc",
                ExitFacts::default(),
            )),
        },
        Event::ExitReviewStarted {
            turn_id: turn(),
            call_id: call(),
            kinds: vec![ExitKind::Persistence],
        },
        judged(call(), JudgeKind::Classifier, Verdict::Allow, Some("you named your zshrc")),
        Event::HomeFileChanged {
            turn_id: turn(),
            call_id: call(),
            path: PathBuf::from("/home/user/.zshrc"),
            floor: true,
            covered: false,
            backup: Some(PathBuf::from("/home/user/.local/share/efr/file-backups/c1/t1/1/.zshrc")),
            real: None,
        },
        Event::ExitRequested {
            turn_id: turn(),
            call_id: second,
            kinds: vec![ExitKind::Upload],
            grants: Vec::new(),
            source: ExitSource::Predicted,
            record: Box::new(exit_record("git push", ExitFacts::default())),
        },
        judged(
            second,
            JudgeKind::Classifier,
            Verdict::Deny,
            Some("no message of yours names a push"),
        ),
        Event::ClassifierPaused { turn_id: turn(), reason: PauseReason::InARow, denials: 3 },
        Event::PresenceChanged {
            conversation_id: conversation(),
            presence: Presence::Away,
            source: PresenceSource::User,
        },
        Event::ApprovalExpired { turn_id: turn(), call_id: second, away: true },
        judged(second, JudgeKind::PresenceExpired, Verdict::Deny, None),
        Event::CacheDeleteApplied {
            turn_id: turn(),
            call_id: call(),
            paths: vec![PathBuf::from("/home/user/.cache/go-build/ab")],
            freed_bytes: 52_000_000,
        },
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn history_shows_reviewer_and_backup_lines() {
    let page = ConversationHistoryResult { events: reviewer_events(), next_cursor: None };
    let options = RenderOptions::new(60).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    // NOTE: the backup line stays whole at 60 columns: it names where the copy is.
    assert!(
        plain.contains(
            "changed ~/.zshrc; backup at ~/.local/share/efr/file-backups/c1/t1/1/.zshrc\n"
        ),
        "{plain}"
    );
    assert!(!plain.contains("gpt-6-luna"), "{plain}");
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
    insta::assert_snapshot!(format!("{plain}---\n{verbose}"));
}

/// A turn of `auto` in phase E: the user answers `a` to a write question, the rule is
/// saved, and a later call runs by that rule with no question.
fn always_events() -> Vec<EventEnvelope> {
    let second: efr_protocol::CallId = "019a9b1c-3d00-7a10-8b20-0000000000b2".parse().unwrap();
    let write = efr_protocol::Grant::Write { path: PathBuf::from("/home/user/notes") };
    let events = vec![
        Event::ApprovalRequested {
            turn_id: turn(),
            call_id: call(),
            summary: "write ~/notes/a".to_owned(),
            diff_preview: None,
            interactive: false,
            exit: Some(ExitInfo {
                always: Some("write under ~/notes".to_owned()),
                ..exit_info(&[ExitKind::Write], Launch::Contained { grants: vec![write] })
            }),
            expires_in_secs: None,
        },
        Event::ApprovalResolved {
            turn_id: turn(),
            call_id: call(),
            decision: ApprovalDecision::AllowAlways,
            origin: Origin::Shell,
        },
        Event::AlwaysRuleSaved {
            turn_id: turn(),
            call_id: call(),
            outcome: efr_protocol::AlwaysSaved::Saved {
                index: 2,
                rule: "write under ~/notes".to_owned(),
            },
        },
        Event::ExitJudged {
            turn_id: turn(),
            call_id: second,
            judge: JudgeKind::Always,
            verdict: Verdict::Allow,
            model: None,
            latency_ms: None,
            risk: None,
            user_authorization: None,
            category: Some("sandbox.allow[2]".to_owned()),
            rationale: Some("write under ~/notes".to_owned()),
            record_sha256: None,
            cached: false,
            chain_step: None,
            skipped: Vec::new(),
            reasked: false,
        },
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn history_shows_judge_always_with_the_rule() {
    let page = ConversationHistoryResult { events: always_events(), next_cursor: None };
    let options = RenderOptions::new(80).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    assert!(plain.contains("allowed always from the shell\n"), "{plain}");
    assert!(plain.contains("saved as sandbox.allow[2]: write under ~/notes\n"), "{plain}");
    assert!(
        plain.contains("allowed by your rule sandbox.allow[2]: write under ~/notes\n"),
        "{plain}"
    );
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
    assert!(
        verbose.contains("exit allowed by an always-allow rule: sandbox.allow[2]\n"),
        "{verbose}"
    );
}

#[test]
fn history_verbose_shows_exit_allowed_by_full_mode() {
    let write = efr_protocol::Grant::Write { path: PathBuf::from("/home/user/notes.txt") };
    let events = vec![
        Event::ExitRequested {
            turn_id: turn(),
            call_id: call(),
            kinds: vec![ExitKind::Write],
            grants: vec![write],
            source: ExitSource::Predicted,
            record: Box::new(exit_record("echo x > ~/notes.txt", ExitFacts::default())),
        },
        Event::ExitJudged {
            turn_id: turn(),
            call_id: call(),
            judge: JudgeKind::Full,
            verdict: Verdict::Allow,
            model: None,
            latency_ms: None,
            risk: None,
            user_authorization: None,
            category: None,
            rationale: None,
            record_sha256: None,
            cached: false,
            chain_step: None,
            skipped: Vec::new(),
            reasked: false,
        },
    ];
    let events = events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let options = RenderOptions::new(80).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
    assert!(verbose.contains("exit allowed by full mode\n"), "{verbose}");
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    assert!(!plain.contains("exit allowed"), "{plain}");
}

/// A turn of `auto` in phase B: efr copies a floor file into its home store, takes a
/// snapshot before and after, and a later `efr undo` takes the turn back.
fn undo_events() -> Vec<EventEnvelope> {
    let undo_id = "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let snapshot = |phase, commit: &str| Event::SnapshotTaken {
        turn_id: turn(),
        root: "project".to_owned(),
        phase,
        commit: commit.to_owned(),
        files: 120,
        skipped: 3,
        ms: 45,
    };
    let events = vec![
        Event::HomeFileChanged {
            turn_id: turn(),
            call_id: call(),
            path: PathBuf::from("/home/user/.zshrc"),
            floor: true,
            covered: true,
            backup: None,
            real: None,
        },
        snapshot(efr_protocol::SnapshotPhase::Pre, "1a2b3c4d5e6f"),
        snapshot(efr_protocol::SnapshotPhase::Post, "5d6e7f8a9b0c"),
        Event::TurnCompleted {
            turn_id: turn(),
            usage: None,
            context: None,
            changes: None,
            stop: None,
            undo: Some(efr_protocol::TurnUndoInfo {
                undoable: true,
                roots: Vec::new(),
                home_files: 1,
                not_covered: vec!["target/ (ignored, large)".to_owned()],
                not_covered_more: 0,
            }),
        },
        Event::UndoApplied {
            conversation_id: conversation(),
            undone_turn_id: turn(),
            undo_id,
            redo: false,
            origin: Origin::Cli,
            restored: 2,
            trashed: 1,
            deleted: 0,
            conflicts: 1,
            not_covered: 0,
            refs: Vec::new(),
            trash: None,
            stopped: false,
        },
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn history_names_a_home_file_that_two_calls_changed_once() {
    let changed = |covered| Event::HomeFileChanged {
        turn_id: turn(),
        call_id: call(),
        path: PathBuf::from("/home/user/.zshrc"),
        floor: true,
        covered,
        backup: None,
        real: None,
    };
    let page = |second_covered| ConversationHistoryResult {
        events: [changed(true), changed(second_covered)]
            .into_iter()
            .zip(1..)
            .map(|(event, seq)| envelope(seq, event))
            .collect(),
        next_cursor: None,
    };
    let options = RenderOptions::new(80).with_terminal(false);
    let shown = |page| {
        transcript(
            conversation(),
            &page,
            &Shown {
                options: &options,
                verbose: false,
                home: Some(Path::new("/home/user")),
                groups: &DiffGroups::default(),
            },
        )
    };
    let covered = shown(page(true));
    assert_eq!(covered.matches("changed ~/.zshrc").count(), 1, "{covered}");
    assert!(covered.contains("changed ~/.zshrc; efr undo reverts it\n"), "{covered}");
    // The second change was not copied: undo cannot revert the file.
    let not_covered = shown(page(false));
    assert_eq!(not_covered.matches("changed ~/.zshrc").count(), 1, "{not_covered}");
    assert!(not_covered.contains("changed ~/.zshrc; efr undo cannot revert it\n"), "{not_covered}");
}

#[test]
fn history_shows_each_undo_as_one_line_and_snapshots_when_verbose() {
    let page = ConversationHistoryResult { events: undo_events(), next_cursor: None };
    let options = RenderOptions::new(80).with_terminal(false);
    let home = Some(Path::new("/home/user"));
    let plain = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: false, home, groups: &DiffGroups::default() },
    );
    for line in [
        "changed ~/.zshrc; efr undo reverts it\n",
        "changed 1 file in ~; efr undo reverts this turn\nnot covered: target/ (ignored, large)\n",
        &format!(
            "undid turn {}: 2 files restored, 1 moved to the undo trash, 1 skipped (changed \
             after the turn)\n",
            turn()
        ),
    ] {
        assert!(plain.contains(line), "{line:?} in {plain}");
    }
    assert!(!plain.contains("snapshot"), "{plain}");
    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home, groups: &DiffGroups::default() },
    );
    assert!(
        verbose.contains(
            "snapshot before the turn of project: 120 files, 3 left out, 45 ms (1a2b3c4)\n"
        ),
        "{verbose}"
    );
    assert!(verbose.contains("snapshot after the turn of project: "), "{verbose}");
}

/// A turn that started job j1, the job's end, and a turn that its notice started.
fn job_events() -> Vec<EventEnvelope> {
    let wake: efr_protocol::TurnId = "019a9b1c-3d00-7a10-8b20-0000000000f2".parse().unwrap();
    let command_id: CommandId = "019a9b1c-3d00-7a10-8b20-0000000000c9".parse().unwrap();
    let notice = |n: u64| {
        Event::JobNotice {
        conversation_id: conversation(),
        turn_id: wake,
        job_id: efr_protocol::JobId::nth(1),
        notice_kind: efr_protocol::NoticeKind::Exited,
        woke: true,
        source_seq: Seq::new(n),
        not_user_input: true,
        text: "<job id=\"j1\" state=\"exited\" exit=\"2\">\nefr: this is a job notice\n\u{1b}[2Jfail \u{202e}x\n</job>".to_owned(),
    }
    };
    let events = vec![
        Event::PromptQueued {
            turn_id: turn(),
            command_id,
            text: "start the build in the background".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
        Event::ToolCallStarted {
            turn_id: turn(),
            call_id: call(),
            tool: "shell".to_owned(),
            input: json!({ "command": "cargo build", "background": true }),
            manual_input: false,
            launch: Some(Launch::contained()),
            freeform: false,
        },
        Event::JobStarted {
            turn_id: turn(),
            call_id: call(),
            job: crate::testing::job(1, "cargo build"),
            launcher_pid: 4242,
            launcher_start: 99,
        },
        Event::JobEnded {
            job_id: efr_protocol::JobId::nth(1),
            state: efr_protocol::JobState::Exited,
            exit_code: Some(2),
            stopped_by: None,
            output_bytes: 2_000,
            ran_secs: 61,
            tail: "error[E0308]: mismatched types\n\u{1b}]0;title\u{7}\u{202e}evil".to_owned(),
            surface_changes: Vec::new(),
            quarantined: false,
            survivors: Vec::new(),
            launcher_lost: false,
            output_truncated: false,
            blocked: Vec::new(),
            blocked_more: 0,
            bus_refused: Vec::new(),
            bus_refused_more: 0,
        },
        notice(4),
        notice(4),
        Event::TurnStarted {
            turn_id: wake,
            cwd: PathBuf::from("/home/user/project"),
            scope: Scope::Machine,
            settings: None,
        },
    ];
    events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect()
}

#[test]
fn wake_turn_marked_in_history() {
    let page = ConversationHistoryResult { events: job_events(), next_cursor: None };
    let options = RenderOptions::new(80).with_terminal(false);
    let plain = show(&page, &options);
    assert!(plain.contains("$ cargo build (background)\n"), "{plain}");
    assert!(
        plain.contains("job j1 started; output $SCRATCH/jobs/j1.out; ends in 180 minutes\n"),
        "{plain}"
    );
    assert!(plain.contains("job j1 ended with exit 2 after 1 minute; 2.0 KB output\n"), "{plain}");
    assert_eq!(
        plain.matches("\u{1f916} (job notice, not your input)\n").count(),
        1,
        "one mark for the turn, however many notices it took: {plain}"
    );
    assert!(!plain.contains("mismatched"), "the tail shows only with --verbose: {plain}");

    let verbose = transcript(
        conversation(),
        &page,
        &Shown { options: &options, verbose: true, home: None, groups: &DiffGroups::default() },
    );
    assert!(verbose.contains("    error[E0308]: mismatched types\n"), "{verbose}");
    assert!(verbose.contains("    efr: this is a job notice\n"), "{verbose}");
    // NOTE: the tail and the notice come from a program: no escape or format character
    // reaches the terminal.
    assert!(!verbose.contains('\u{1b}') && !verbose.contains('\u{202e}'), "{verbose:?}");
    assert!(verbose.contains("\u{241b}]0;title\u{2407}\u{fffd}evil"), "{verbose}");
}

#[test]
fn history_shows_job_port_forwards() {
    let port = |port: u16, outcome: efr_protocol::ForwardOutcome| Event::JobPortForwarded {
        job_id: efr_protocol::JobId::nth(1),
        listen: "::1".to_owned(),
        port,
        outcome,
        by_job: None,
        reason: None,
    };
    let page = ConversationHistoryResult {
        events: vec![
            envelope(1, port(5173, efr_protocol::ForwardOutcome::Forwarded)),
            envelope(2, port(5174, efr_protocol::ForwardOutcome::InUse)),
        ],
        next_cursor: None,
    };
    let plain = show(&page, &RenderOptions::new(80).with_terminal(false));
    assert!(plain.contains("job j1 listens on [::1]:5173 -> http://localhost:5173\n"), "{plain}");
    assert!(
        plain.contains(
            "job j1 listens on [::1]:5174; not forwarded: 127.0.0.1:5174 is in use on this \
             machine\n"
        ),
        "{plain}"
    );
    assert!(!plain.contains('\u{1b}'), "{plain:?}");

    let terminal = show(&page, &RenderOptions::new(80));
    assert_eq!(
        terminal.matches("\u{1b}]8;;http://localhost:5173\u{1b}\\").count(),
        1,
        "{terminal:?}"
    );
    assert!(!terminal.contains("\u{1b}]8;;http://localhost:5174"), "{terminal:?}");
}

#[test]
fn a_turn_that_undo_cannot_take_back_shows_the_line_of_its_files() {
    let file = |path: &str, added: u32, removed: u32| efr_protocol::FileChange {
        path: path.to_owned(),
        kind: efr_protocol::ChangeKind::Modified,
        from: None,
        added,
        removed,
        binary: false,
    };
    let changes = efr_protocol::FileChanges::from_files(vec![
        file("src/a.rs", 3, 1),
        file("tests/a.rs", 7, 0),
    ]);
    let ended = |undo: Option<efr_protocol::TurnUndoInfo>| Event::TurnCompleted {
        turn_id: turn(),
        usage: None,
        context: None,
        changes: Some(changes.clone()),
        stop: None,
        undo,
    };
    let options = RenderOptions::new(80).with_terminal(false);
    let page =
        ConversationHistoryResult { events: vec![envelope(1, ended(None))], next_cursor: None };

    let text = show(&page, &options);

    assert!(text.contains("2 files changed, +10 -1 (tests +7)\n"), "{text}");
    let groups = DiffGroups::new(vec![("checks".to_owned(), vec!["tests/".to_owned()])]);
    let shown = Shown { options: &options, verbose: false, home: None, groups: &groups };
    let text = transcript(conversation(), &page, &shown);
    assert!(text.contains("2 files changed, +10 -1 (checks +7)\n"), "{text}");
    // A turn that changed no file shows no line.
    let none = ConversationHistoryResult {
        events: vec![envelope(
            1,
            Event::TurnCompleted {
                turn_id: turn(),
                usage: None,
                context: None,
                changes: None,
                stop: None,
                undo: None,
            },
        )],
        next_cursor: None,
    };
    assert!(!show(&none, &options).contains("changed"));
}

/// The transcript of `page` without colour, verbose or not.
fn plain(page: &ConversationHistoryResult, verbose: bool) -> String {
    let options = RenderOptions::new(80).with_terminal(false);
    let shown = Shown { options: &options, verbose, home: None, groups: &DiffGroups::default() };
    transcript(conversation(), page, &shown)
}

/// A page of `events`, numbered from 1.
fn page_of(events: Vec<Event>) -> ConversationHistoryResult {
    ConversationHistoryResult {
        events: events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect(),
        next_cursor: None,
    }
}

#[test]
fn interrupted_partial_prints_once() {
    // The live order of an interrupt: the update, the request that flushes it, then the
    // completion of the same text and the end.
    let page = page_of(vec![
        Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            offset: 0,
            delta: "Once upon a time".to_owned(),
        },
        Event::TurnInterruptRequested { turn_id: turn(), origin: Origin::Shell },
        Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "Once upon a time".to_owned(),
        },
        Event::TurnInterrupted {
            turn_id: turn(),
            usage: None,
            context: None,
            changes: None,
            undo: None,
        },
    ]);

    let text = plain(&page, false);

    assert_eq!(text.matches("Once upon a time").count(), 1, "{text}");
    assert!(text.ends_with("Once upon a time\n\ninterrupted\n"), "{text}");
}

#[test]
fn a_completion_with_more_text_than_the_flush_still_prints() {
    let page = page_of(vec![
        Event::AssistantMessageUpdated {
            turn_id: turn(),
            index: 0,
            offset: 0,
            delta: "Once".to_owned(),
        },
        Event::TurnInterruptRequested { turn_id: turn(), origin: Origin::Shell },
        Event::AssistantMessageCompleted {
            turn_id: turn(),
            index: 0,
            text: "Once upon a time".to_owned(),
        },
    ]);

    let text = plain(&page, false);

    assert!(text.contains("Once upon a time\n"), "the whole text is never lost: {text}");
}

#[test]
fn history_verbose_shows_the_reask() {
    let judged = |reasked| Event::ExitJudged {
        turn_id: turn(),
        call_id: call(),
        judge: JudgeKind::Classifier,
        verdict: Verdict::Allow,
        model: Some("gpt-6-luna".to_owned()),
        latency_ms: Some(2_410),
        risk: Some(Risk::Low),
        user_authorization: None,
        category: Some("file edit".to_owned()),
        rationale: Some("you asked to add the alias".to_owned()),
        record_sha256: None,
        cached: false,
        chain_step: Some(ChainStep::ClassifierModel),
        skipped: Vec::new(),
        reasked,
    };

    let verbose = plain(&page_of(vec![judged(true)]), true);
    assert!(
        verbose.contains(
            "  model gpt-6-luna; the reviewer model; 2410 ms; asked again for a real reason\n"
        ),
        "{verbose}"
    );
    assert!(!plain(&page_of(vec![judged(true)]), false).contains("asked again"), "--verbose only");
    assert!(!plain(&page_of(vec![judged(false)]), true).contains("asked again"));
}

#[test]
fn history_verbose_shows_usage_per_turn() {
    let second: efr_protocol::TurnId = "019a9b1c-3d00-7a10-8b20-0000000000f2".parse().unwrap();
    let usage = efr_protocol::Usage {
        cached_input_tokens: 16_600,
        cache_write_tokens: 1_200,
        cache_write_1h_tokens: 1_200,
        ..efr_protocol::Usage::new(18_200, 1_100)
    };
    let page = page_of(vec![
        Event::TurnCompleted {
            turn_id: turn(),
            usage: Some(usage),
            context: None,
            changes: None,
            stop: None,
            undo: None,
        },
        Event::TurnInterrupted {
            turn_id: second,
            usage: Some(efr_protocol::Usage::new(900, 12)),
            context: None,
            changes: None,
            undo: None,
        },
    ]);

    let verbose = plain(&page, true);
    assert!(verbose.contains("18.2k in, 1.1k out, cache 91%, wrote 1.2k (1h)\n"), "{verbose}");
    assert!(verbose.contains("interrupted\n900 in, 12 out\n"), "interrupted too: {verbose}");
    let quiet = plain(&page, false);
    assert!(!quiet.contains(" in, "), "only --verbose shows the tokens: {quiet}");
}

#[test]
fn history_says_who_moved_a_call_to_a_job_and_what_the_move_covered() {
    let mut job = crate::testing::job(3, "make deploy");
    job.ends_by = job.started_at + jiff::SignedDuration::from_mins(60);
    let mut exit = exit_info(&[ExitKind::Write], Launch::Direct);
    exit.facts = vec!["a job asks in cautious".to_owned()];
    job.moved_from = Some(efr_protocol::MovedCall {
        call_id: call(),
        turn_id: turn(),
        after_secs: 42,
        exits: vec![exit],
    });
    let events = vec![
        Event::ToolCallStarted {
            turn_id: turn(),
            call_id: call(),
            tool: "shell".to_owned(),
            input: json!({ "command": "make deploy" }),
            manual_input: false,
            launch: None,
            freeform: false,
        },
        Event::JobStarted {
            turn_id: turn(),
            call_id: call(),
            job,
            launcher_pid: 4242,
            launcher_start: 99,
        },
    ];
    let page = ConversationHistoryResult {
        events: events.into_iter().zip(1..).map(|(event, seq)| envelope(seq, event)).collect(),
        next_cursor: None,
    };
    let plain = show(&page, &RenderOptions::new(100).with_terminal(false));
    assert!(
        plain.contains(
            "moved to job j3 by you after 42 seconds; output $SCRATCH/jobs/j3.out; ends in 60 \
             minutes\n"
        ),
        "{plain}"
    );
    assert!(plain.contains("your move covered: write; efr: a job asks in cautious\n"), "{plain}");
}
