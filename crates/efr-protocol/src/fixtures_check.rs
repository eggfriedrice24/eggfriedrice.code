//! The samples behind the frozen fixtures in `fixtures/v1/`, and their canonical form.
//!
//! The fixture files are the contract that the phone repository reads, so they are
//! plain JSON compared byte for byte (the README says why not `insta`). Each sample here
//! must encode to exactly the bytes of its file, and each file must decode as its type
//! and encode back to the same bytes; the tests are in `fixtures_check/tests.rs`.
//! Samples set every optional member they can, so the files freeze every member's form.
//! Objects built with `json!` list their keys in sorted order, so the files are the same
//! whether or not serde_json's `preserve_order` feature is on.

use std::str::FromStr;

use jiff::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, json};

use crate::{
    ActionFacts, AdminConfigReload, AdminConfigReloadResult, AdminLoginOpenAi,
    AdminLoginOpenAiItem, AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove,
    AdminProjectRemoveResult, AdminSandboxCheck, AdminSandboxCheckResult, AdminStatus,
    AdminStatusResult, ApprovalDecision, ApprovalRespond, ApprovalRespondResult, Base64Bytes,
    BlockReason, Blocked, BusKind, CacheMode, CallId, Capabilities, Cell, ChangeKind, CheckOutcome,
    ClientFrame, Color, CommandId, Compaction, CompactionId, CompactionTrigger, ConfigFileError,
    ConfigStatus, ContextUse, ConversationCompact, ConversationCompactResult, ConversationDiff,
    ConversationDiffResult, ConversationHistory, ConversationHistoryResult, ConversationId,
    ConversationSnapshot, ConversationStatus, ConversationSubscribe, ConversationSubscribeItem,
    ConversationSummary, ConversationsList, ConversationsListResult, Cursor, DaemonId, DaemonPaths,
    DaemonRoots, DeviceId, Draft, DraftPart, EffectiveSettings, ErrorBody, ErrorCode, Event,
    EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitRecord, ExitSource, FileChange, FileChanges,
    GitCounts, Grant, Hello, HelloResult, HostFact, InputRespond, InputRespondResult, InputWait,
    JudgeKind, Judgement, LateSteer, Launch, LeaseReport, LeaseReportResult, Method, Mode,
    ModeFallback, ModelInfo, ModelSource, ModelsList, ModelsListResult, NetworkMode, Origin,
    OverriddenSettings, PROTOCOL_VERSION, PageCursor, PathClassName, ProgramFact, ProjectId,
    ProjectInfo, ProjectsList, ProjectsListResult, PromptSend, PromptSendResult, PromptWithdraw,
    PromptWithdrawResult, ProviderStatus, PtyAttach, PtyAttachItem, PtyId, PtyResize,
    PtyResizeResult, PtyWrite, PtyWriteResult, QuestionId, ReportedFile, RequestId, ResentSteers,
    Risk, RootDir, RootSource, RowCells, SandboxCheck, SandboxExplain, SandboxExplainResult,
    SandboxPathRole, SandboxPaths, SandboxStatus, SandboxSummary, SandboxSurfaceRespond,
    SandboxSurfaceRespondResult, Scope, ScopeName, ScreenSnapshot, SecretText, Seq, ServerFrame,
    ShellContext, Size, SurfaceChange, TargetFact, TurnId, TurnInterrupt, TurnInterruptResult,
    TurnSettings, TurnSteer, TurnSteerResult, Usage, UserAuthorization, Verdict, WithdrawTarget,
    WithdrawnPrompt, WithdrawnSteer,
};

/// The directory of the frozen fixtures.
pub(crate) const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/v1");

/// One frozen fixture: where it lives, what its file must hold, and how to read it back.
pub(crate) struct Fixture {
    /// The path under `fixtures/v1/`, with `/` separators.
    pub(crate) path: String,
    /// The canonical encoding of the sample: the exact bytes of the file.
    pub(crate) json: String,
    /// Decodes a file as the sample's type and encodes it again.
    pub(crate) round_trip: fn(&str) -> Result<String, serde_json::Error>,
}

/// The canonical form of a fixture: pretty JSON with two-space indents and a final
/// newline.
pub(crate) fn canonical<T: Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

fn round_trip<T: Serialize + DeserializeOwned>(text: &str) -> Result<String, serde_json::Error> {
    let value: T = serde_json::from_str(text)?;
    canonical(&value)
}

fn fixture<T: Serialize + DeserializeOwned>(path: impl Into<String>, sample: &T) -> Fixture {
    let path = path.into();
    let json =
        canonical(sample).unwrap_or_else(|err| panic!("sample {path} does not encode: {err}"));
    Fixture { path, json, round_trip: round_trip::<T> }
}

/// The file name stem of a method, such as `conversation_subscribe`.
pub(crate) fn method_stem(method: &Method) -> String {
    method.name().replace('.', "_")
}

/// The file of an event sample under `fixtures/v1/`.
pub(crate) fn event_path(event: &Event) -> String {
    match event {
        Event::Unknown { .. } => "events/unknown.json".to_owned(),
        known => format!("events/{}.json", known.kind()),
    }
}

/// Every fixture, in a stable order.
pub(crate) fn all() -> Vec<Fixture> {
    let mut fixtures: Vec<Fixture> = method_samples()
        .iter()
        .map(|method| fixture(format!("{}_params.json", method_stem(method)), method))
        .collect();
    fixtures.extend(answer_fixtures());
    fixtures.extend(event_samples().iter().map(|event| fixture(event_path(event), event)));
    fixtures.push(fixture("events/tool_call_started_freeform.json", &freeform_call_sample()));
    fixtures.extend(frame_fixtures());
    fixtures.push(fixture("input_waits.json", &input_wait_samples()));
    fixtures.push(fixture("modes.json", &Mode::ALL.to_vec()));
    fixtures.push(fixture("error_codes.json", &ErrorCode::ALL.to_vec()));
    fixtures.push(fixture("scope_names.json", &ScopeName::ALL.to_vec()));
    fixtures.push(fixture("exit_kinds.json", &ExitKind::ALL.to_vec()));
    fixtures.push(fixture("grants.json", &grant_samples()));
    fixtures.push(fixture("launches.json", &launch_samples()));
    fixtures.push(fixture("draft_parts.json", &draft_part_samples()));
    fixtures.push(fixture("withdraw_targets.json", &withdraw_target_samples()));
    fixtures
}

/// A `tool_call_started` of a freeform tool, whose input is text. The sample of
/// `events/tool_call_started.json` is a `shell` call, whose input is JSON.
pub(crate) fn freeform_call_sample() -> Event {
    Event::ToolCallStarted {
        turn_id: turn_id(),
        call_id: call_id(),
        tool: "apply_patch".into(),
        input: json!(
            "*** Begin Patch\n*** Update File: src/main.rs\n@@ fn main\n-    println!(\"hi\");\n+    println!(\"hello\");\n*** End Patch\n"
        ),
        freeform: true,
        manual_input: false,
        launch: None,
    }
}

/// One sample of every kind of target of `prompt.withdraw`.
pub(crate) fn withdraw_target_samples() -> Vec<WithdrawTarget> {
    vec![
        WithdrawTarget::Turn { turn_id: queued_turn_id() },
        WithdrawTarget::NewestFromTty { tty: "/dev/pts/3".to_owned() },
    ]
}

/// A prompt that `prompt.withdraw` or `turn.interrupt` took back.
fn withdrawn_prompt() -> WithdrawnPrompt {
    WithdrawnPrompt {
        turn_id: queued_turn_id(),
        seq: Seq::new(47),
        text: "then clean up the old logs".to_owned(),
    }
}

/// One sample of every kind of draft part.
pub(crate) fn draft_part_samples() -> Vec<DraftPart> {
    vec![
        DraftPart::Text { index: 0, offset: 49, delta: " The largest part is".to_owned() },
        DraftPart::Reasoning {
            offset: 0,
            delta: "**Reading the journal size**\n\nThe user asks".to_owned(),
            title: Some("Reading the journal size".to_owned()),
        },
        DraftPart::ToolInput { call: 0, tool: "write_file".to_owned(), bytes: 3277 },
        DraftPart::Context(context_use()),
        DraftPart::Compacting { trigger: CompactionTrigger::Auto },
    ]
}

/// How full the context of [`turn_id`] is: 43% of the trigger of a 272k window.
fn context_use() -> ContextUse {
    ContextUse { tokens: 89_000, limit: 206_720, window: 272_000 }
}

fn compaction_id() -> CompactionId {
    parse("019a9b1c-3d00-7a10-8b20-00000000000b")
}

/// A compaction that pruned and then wrote a summary, inside [`turn_id`].
fn compaction() -> Compaction {
    Compaction {
        compaction_id: compaction_id(),
        turn_id: Some(turn_id()),
        trigger: CompactionTrigger::Auto,
        focus: None,
        model: "gpt-5.5".into(),
        window: 272_000,
        limit: 206_720,
        tokens_before: 231_000,
        tokens_after: 24_000,
        through_turn: turn_id(),
        through_message: Some(6),
        kept_turns: 3,
        pruned_outputs: 12,
        pruned_tokens: 41_000,
        omitted_turns: 0,
        omitted_messages: 0,
        summary: Some("## Task and state\nFree space on /var.".into()),
        usage: Some(Usage {
            input_tokens: 205_000,
            output_tokens: 3_200,
            cached_input_tokens: 198_000,
            reasoning_tokens: 900,
            context_tokens: 208_200,
        }),
    }
}

fn parse<T: FromStr>(text: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    text.parse().unwrap_or_else(|err| panic!("{text} does not parse: {err:?}"))
}

fn conversation_id() -> ConversationId {
    parse("019a9b1c-3d00-7a10-8b20-000000000001")
}

fn turn_id() -> TurnId {
    parse("019a9b1c-3d00-7a10-8b20-000000000002")
}

/// A second turn: one that waits in the queue behind [`turn_id`].
fn queued_turn_id() -> TurnId {
    parse("019a9b1c-3d00-7a10-8b20-00000000000a")
}

fn command_id() -> CommandId {
    parse("019a9b1c-3d00-7a10-8b20-000000000003")
}

fn call_id() -> CallId {
    parse("019a9b1c-3d00-7a10-8b20-000000000004")
}

fn pty_id() -> PtyId {
    parse("019a9b1c-3d00-7a10-8b20-000000000005")
}

fn device_id() -> DeviceId {
    parse("019a9b1c-3d00-7a10-8b20-000000000006")
}

fn daemon_id() -> DaemonId {
    parse("019a9b1c-3d00-7a10-8b20-000000000007")
}

fn project_id() -> ProjectId {
    parse("019a9b1c-3d00-7a10-8b20-000000000008")
}

fn question_id() -> QuestionId {
    parse("019a9b1c-3d00-7a10-8b20-000000000009")
}

fn at(text: &str) -> Timestamp {
    parse(text)
}

fn shell_context() -> ShellContext {
    ShellContext {
        pwd: "/var/log".into(),
        oldpwd: Some("/home/me".into()),
        tty: Some("/dev/pts/3".into()),
        shell_pid: Some(4242),
        last_status: Some(1),
        shlvl: Some(1),
        ssh_connection: Some("192.0.2.10 51234 192.0.2.20 22".into()),
        hostname: Some("desk".into()),
    }
}

fn turn_settings() -> TurnSettings {
    TurnSettings {
        mode: Some(Mode::Auto),
        model: Some("gpt-5.4".into()),
        effort: Some("high".into()),
    }
}

/// What [`turn_settings`] runs with: every value comes from the prompt.
fn effective_settings() -> EffectiveSettings {
    EffectiveSettings {
        mode: Mode::Auto,
        model: "gpt-5.4".into(),
        effort: Some("high".into()),
        overridden: OverriddenSettings { mode: true, model: true, effort: true },
        fallback: None,
    }
}

/// An `auto` turn that runs as `cautious`, so `events/turn_started.json` freezes the
/// fallback.
fn fallen_back_settings() -> EffectiveSettings {
    EffectiveSettings {
        mode: Mode::Cautious,
        fallback: Some(ModeFallback {
            asked: Mode::Auto,
            reason: "Landlock ABI 6 found; auto needs 9 (Linux 7.1)".into(),
        }),
        ..effective_settings()
    }
}

/// One grant of every kind, so `grants.json` freezes the wire form of each.
pub(crate) fn grant_samples() -> Vec<Grant> {
    vec![
        Grant::Write { path: "/home/me/notes".into() },
        Grant::Host { host: "registry.npmjs.org".into(), port: 443 },
        Grant::OpenNetwork,
        Grant::Socket { path: "/run/user/1000/app.sock".into() },
        Grant::Bus { bus: BusKind::Session },
        Grant::Bus { bus: BusKind::System },
        Grant::Device { path: "/dev/nvme0n1".into() },
        Grant::Unmask { path: "/home/me/p/app/.env".into() },
    ]
}

/// Every launch, so `launches.json` freezes the wire form of each.
pub(crate) fn launch_samples() -> Vec<Launch> {
    vec![
        Launch::Direct,
        Launch::contained(),
        Launch::Contained { grants: vec![Grant::OpenNetwork] },
        Launch::Unsandboxed,
    ]
}

/// A probe that found everything ready, with a warning.
fn sandbox_status() -> SandboxStatus {
    SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(15),
        bwrap: Some("/usr/bin/bwrap".into()),
        bwrap_version: Some("0.13.0".into()),
        cache_mode: CacheMode::Overlay,
        network_mode: NetworkMode::None,
        warnings: vec![
            "the registered project ~ is your home directory; auto runs its turns as cautious"
                .into(),
        ],
    }
}

fn surface_change() -> SurfaceChange {
    SurfaceChange {
        path: "/home/me/p/app/.git/commondir".into(),
        rule: "commondir_in_main_git_dir".into(),
        key: Some("core.fsmonitor".into()),
        quarantined: true,
    }
}

/// A record with every member set.
fn exit_record() -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["copy the report to ~/Documents".into()],
        action: ActionFacts {
            tool: "shell".into(),
            line: "cp report.pdf ~/Documents/".into(),
            cwd: "/home/me/p/app".into(),
            scope: Scope::Project(project_id()),
            exits: vec![ExitKind::Write],
            grants: vec![Grant::Write { path: "/home/me/Documents".into() }],
            source: ExitSource::Predicted,
        },
        facts: ExitFacts {
            targets: vec![TargetFact {
                path: "/home/me/Documents".into(),
                class: Some(PathClassName::UserData),
                in_write_root: false,
                floor: false,
                synced: false,
                exists: true,
                named_in_user_messages: true,
            }],
            hosts: vec![HostFact {
                host: "example.com".into(),
                on_allow_list: false,
                named_in_user_messages: false,
                refused_by_proxy: true,
            }],
            programs: vec![ProgramFact {
                word: "cp".into(),
                resolved: Some("/usr/bin/cp".into()),
                in_write_root: false,
                changed_this_turn: false,
            }],
            upload_patterns: vec!["git push".into()],
            repo_surface_changed_this_turn: true,
            git_status: Some(GitCounts { modified: 3, untracked: 1, staged: 0 }),
            snapshot_covers: Some(false),
            sandbox_export_names: vec!["VIRTUAL_ENV".into()],
            previous_exits_this_turn: vec![(ExitKind::Host, Verdict::Allow)],
            refusals_in_a_row: 1,
        },
    }
}

fn judgement() -> Judgement {
    Judgement {
        verdict: Verdict::AskUser,
        risk: Risk::High,
        user_authorization: UserAuthorization::Medium,
        category: "action outside the user's request".into(),
        rationale: "no user message names ~/Documents".into(),
    }
}

/// Each root from another source, so `admin_status_result.json` freezes every
/// [`RootSource`].
pub(crate) fn daemon_roots() -> DaemonRoots {
    DaemonRoots {
        config: RootDir { path: "/home/me/.config/efr".into(), source: RootSource::Xdg },
        data: RootDir { path: "/home/me/efr/data".into(), source: RootSource::EfrHome },
        state: RootDir { path: "/var/tmp/efr-state".into(), source: RootSource::DirVariable },
        runtime: RootDir { path: "/run/user/1000/efr".into(), source: RootSource::RunUser },
    }
}

/// A broken rule: the config's error has every member.
fn config_file_error() -> ConfigFileError {
    ConfigFileError {
        message: "unknown variant `maybe`, expected one of `allow`, `ask`, `deny`".into(),
        line: Some(42),
        column: Some(10),
        key: Some("permissions.rules[2].effect".into()),
    }
}

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: conversation_id(),
        title: Some("why is the disk full".into()),
        status: ConversationStatus::Running,
        created_at: at("2026-10-03T11:58:00Z"),
        updated_at: at("2026-10-03T12:00:05.5Z"),
        last_seq: Seq::new(42),
        cwd: Some("/var/log".into()),
        scope: Some(Scope::Machine),
        tty: Some("/dev/pts/3".into()),
    }
}

fn envelope(seq: u64, event: Event) -> EventEnvelope {
    EventEnvelope {
        seq: Seq::new(seq),
        conversation_id: Some(conversation_id()),
        at: at("2026-10-03T12:00:05Z"),
        event,
    }
}

fn message_updated() -> Event {
    Event::AssistantMessageUpdated {
        turn_id: turn_id(),
        index: 0,
        offset: 12,
        delta: "takes 3.1 GiB under /var/log/journal.".into(),
    }
}

fn plain(text: &str) -> Cell {
    Cell { text: text.into(), ..Cell::default() }
}

fn snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        size: Size { cols: 80, rows: 2 },
        cursor: Cursor { row: 1, col: 2, hidden: false },
        rows: vec![
            RowCells {
                cells: vec![
                    Cell {
                        text: "o".into(),
                        fg: Some(Color::Indexed(2)),
                        bg: Some(Color::Rgb([30, 30, 46])),
                        bold: true,
                        italic: true,
                        underline: true,
                        inverse: true,
                        wide: false,
                    },
                    plain("k"),
                ],
                wrapped: true,
            },
            RowCells {
                cells: vec![plain("$"), plain(" "), Cell { wide: true, ..plain("界") }, plain("")],
                wrapped: false,
            },
        ],
        scrollback: vec![RowCells { cells: vec![plain("$")], wrapped: false }],
        title: Some("zsh".into()),
        alternate_screen: true,
    }
}

/// One params sample per method.
pub(crate) fn method_samples() -> Vec<Method> {
    vec![
        Method::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            origin: Origin::Shell,
            client: Some("efr 0.1.0".into()),
            capabilities: Capabilities {
                admin: None,
                screen_snapshots: Some(true),
                extra: Default::default(),
            },
            tty: Some("/dev/pts/3".into()),
            pid: Some(5150),
            device_id: Some(device_id()),
        }),
        Method::ConversationsList(ConversationsList {
            cursor: Some(PageCursor::new("c1:019a9b1c-3d00-7a10-8b20-000000000001")),
            limit: Some(20),
        }),
        Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id: conversation_id(),
            after_seq: Some(Seq::new(40)),
            answers_input: true,
            drafts: true,
        }),
        Method::ConversationHistory(ConversationHistory {
            conversation_id: conversation_id(),
            cursor: Some(PageCursor::new("h1:30")),
            limit: Some(50),
        }),
        Method::PromptSend(PromptSend {
            command_id: command_id(),
            conversation_id: Some(conversation_id()),
            new_conversation: false,
            text: "why is the disk full".into(),
            context: Some(shell_context()),
            last_command: Some("du -sh /var/log".into()),
            settings: turn_settings(),
        }),
        Method::TurnInterrupt(TurnInterrupt {
            command_id: command_id(),
            conversation_id: conversation_id(),
            turn_id: Some(turn_id()),
            resend_steers: vec![Seq::new(45)],
            resend_as: Some(Box::new(LateSteer::Queue {
                context: Some(shell_context()),
                last_command: Some("du -sh /var/log".into()),
                settings: turn_settings(),
            })),
            withdraw_steers: vec![Seq::new(46)],
            withdraw: vec![queued_turn_id()],
        }),
        Method::TurnSteer(TurnSteer {
            command_id: command_id(),
            conversation_id: conversation_id(),
            turn_id: Some(turn_id()),
            text: "check the journal too".into(),
            if_late: Some(LateSteer::Queue {
                context: Some(shell_context()),
                last_command: Some("du -sh /var/log".into()),
                settings: turn_settings(),
            }),
        }),
        Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(),
            conversation_id: conversation_id(),
            call_id: call_id(),
            decision: ApprovalDecision::Allow,
        }),
        Method::PtyAttach(PtyAttach {
            pty_id: pty_id(),
            since_seq: Some(Seq::new(1024)),
            scrollback_rows: Some(200),
        }),
        Method::PtyWrite(PtyWrite {
            pty_id: pty_id(),
            data: Base64Bytes::new(b"ls -la\r".to_vec()),
        }),
        Method::PtyResize(PtyResize { pty_id: pty_id(), size: Size { cols: 120, rows: 40 } }),
        Method::InputRespond(InputRespond {
            conversation_id: conversation_id(),
            call_id: call_id(),
            text: SecretText::new("hunter2"),
            hidden: false,
            manual: true,
        }),
        Method::LeaseReport(LeaseReport {
            conversations: vec![conversation_id()],
            ptys: vec![pty_id()],
            visible: true,
        }),
        Method::ModelsList(ModelsList {}),
        Method::ProjectsList(ProjectsList {}),
        Method::AdminProjectAdd(AdminProjectAdd {
            path: "/home/me/p/app/src".into(),
            name: Some("app".into()),
            git_root: true,
        }),
        Method::AdminProjectRemove(AdminProjectRemove { path: "/home/me/p/app".into() }),
        Method::AdminStatus(AdminStatus {}),
        Method::AdminConfigReload(AdminConfigReload {}),
        Method::AdminLoginOpenAi(AdminLoginOpenAi {}),
        Method::SandboxExplain(SandboxExplain {
            path: "../.zshrc".into(),
            cwd: Some("/home/me/p/app".into()),
        }),
        Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
            command_id: command_id(),
            conversation_id: conversation_id(),
            question_id: question_id(),
            keep: false,
        }),
        Method::AdminSandboxCheck(AdminSandboxCheck {}),
        Method::ConversationDiff(ConversationDiff {
            conversation_id: Some(conversation_id()),
            turn_id: Some(turn_id()),
            stat: true,
        }),
        Method::PromptWithdraw(PromptWithdraw {
            command_id: command_id(),
            conversation_id: conversation_id(),
            target: WithdrawTarget::Turn { turn_id: queued_turn_id() },
        }),
        Method::ConversationCompact(ConversationCompact {
            command_id: command_id(),
            conversation_id: conversation_id(),
            focus: Some("the journal cleanup".into()),
        }),
    ]
}

/// A list with every kind of change and every member set.
fn file_changes() -> FileChanges {
    FileChanges {
        files: vec![
            FileChange {
                path: "src/a.rs".into(),
                kind: ChangeKind::Modified,
                from: None,
                added: 3,
                removed: 1,
                binary: false,
            },
            FileChange {
                path: "notes.md".into(),
                kind: ChangeKind::Added,
                from: None,
                added: 12,
                removed: 0,
                binary: false,
            },
            FileChange {
                path: "old.rs".into(),
                kind: ChangeKind::Deleted,
                from: None,
                added: 0,
                removed: 40,
                binary: false,
            },
            FileChange {
                path: "src/new_name.rs".into(),
                kind: ChangeKind::Renamed,
                from: Some("src/old_name.rs".into()),
                added: 0,
                removed: 0,
                binary: false,
            },
            FileChange {
                path: "$SCRATCH/plot.png".into(),
                kind: ChangeKind::Added,
                from: None,
                added: 0,
                removed: 0,
                binary: true,
            },
        ],
        more: 2,
        added: 24,
        removed: 47,
    }
}

/// The result of every unary method and each item variant of every streaming method.
fn answer_fixtures() -> Vec<Fixture> {
    vec![
        fixture(
            "hello_result.json",
            &HelloResult {
                daemon_id: daemon_id(),
                protocol: PROTOCOL_VERSION,
                version: "0.1.0".into(),
                capabilities: Capabilities {
                    admin: Some(true),
                    screen_snapshots: Some(true),
                    extra: Default::default(),
                },
                paths: DaemonPaths {
                    scratch_root: "/home/me/.local/share/efr/scratch".into(),
                    data_dir: "/home/me/.local/share/efr".into(),
                },
                challenge: "3q2-7wAAAAAAAAAAAAAAAA".into(),
            },
        ),
        fixture(
            "conversations_list_result.json",
            &ConversationsListResult {
                conversations: vec![summary()],
                next_cursor: Some(PageCursor::new("c1:019a9b1c-3d00-7a10-8b20-000000000001")),
            },
        ),
        fixture(
            "conversation_subscribe_item_event.json",
            &ConversationSubscribeItem::Event(envelope(41, message_updated())),
        ),
        fixture(
            "conversation_subscribe_item_snapshot.json",
            &ConversationSubscribeItem::Snapshot(ConversationSnapshot {
                conversation: summary(),
                events: vec![envelope(42, message_updated())],
                history_cursor: Some(PageCursor::new("h1:42")),
                hwm: Seq::new(42),
            }),
        ),
        fixture(
            "conversation_subscribe_item_draft.json",
            &ConversationSubscribeItem::Draft(Draft {
                turn_id: turn_id(),
                after_seq: Seq::new(41),
                draft: DraftPart::Text {
                    index: 0,
                    offset: 49,
                    delta: " The largest part is".to_owned(),
                },
            }),
        ),
        fixture(
            "conversation_history_result.json",
            &ConversationHistoryResult {
                events: vec![envelope(
                    30,
                    Event::TurnCompleted {
                        turn_id: turn_id(),
                        usage: Some(Usage::new(1200, 340)),
                        context: None,
                        changes: None,
                    },
                )],
                next_cursor: Some(PageCursor::new("h1:10")),
            },
        ),
        fixture(
            "prompt_send_result.json",
            &PromptSendResult {
                conversation_id: conversation_id(),
                turn_id: turn_id(),
                seq: Seq::new(43),
                queued: false,
                settings: Some(effective_settings()),
            },
        ),
        fixture(
            "turn_interrupt_result.json",
            &TurnInterruptResult {
                turn_id: turn_id(),
                seq: Seq::new(44),
                resent: Some(ResentSteers {
                    turn_id: queued_turn_id(),
                    seq: Seq::new(48),
                    steers: vec![Seq::new(45)],
                }),
                withdrawn: vec![withdrawn_prompt()],
                withdrawn_steers: vec![WithdrawnSteer {
                    seq: Seq::new(46),
                    text: "and the rotated files".to_owned(),
                }],
            },
        ),
        fixture(
            "turn_steer_result.json",
            &TurnSteerResult { turn_id: queued_turn_id(), seq: Seq::new(49), queued: true },
        ),
        fixture(
            "prompt_withdraw_result.json",
            &PromptWithdrawResult { withdrawn: withdrawn_prompt() },
        ),
        fixture(
            "conversation_compact_result.json",
            &ConversationCompactResult {
                seq: Seq::new(50),
                compaction: Compaction {
                    turn_id: None,
                    trigger: CompactionTrigger::Manual,
                    focus: Some("the journal cleanup".into()),
                    through_message: None,
                    ..compaction()
                },
            },
        ),
        fixture("approval_respond_result.json", &ApprovalRespondResult { seq: Seq::new(46) }),
        fixture(
            "pty_attach_item_snapshot.json",
            &PtyAttachItem::Snapshot { seq: Seq::new(2048), snapshot: snapshot() },
        ),
        fixture(
            "pty_attach_item_output.json",
            &PtyAttachItem::Output {
                seq: Seq::new(2048),
                data: Base64Bytes::new(b"\x1b[32mok\x1b[0m\r\n".to_vec()),
            },
        ),
        fixture(
            "pty_attach_item_resized.json",
            &PtyAttachItem::Resized { seq: Seq::new(2060), size: Size { cols: 120, rows: 40 } },
        ),
        fixture("pty_write_result.json", &PtyWriteResult {}),
        fixture("pty_resize_result.json", &PtyResizeResult { size: Size { cols: 120, rows: 40 } }),
        fixture("input_respond_result.json", &InputRespondResult {}),
        fixture("lease_report_result.json", &LeaseReportResult { ttl_secs: 45 }),
        fixture("models_list_result.json", &models_list_sample()),
        fixture(
            "projects_list_result.json",
            &ProjectsListResult {
                file: "/home/me/.config/efr/projects.toml".into(),
                projects: vec![
                    project_info(),
                    ProjectInfo { id: other_project_id(), root: "/etc/nixos".into(), name: None },
                ],
            },
        ),
        fixture(
            "admin_project_add_result.json",
            &AdminProjectAddResult {
                project: project_info(),
                file: "/home/me/dotfiles/efr/projects.toml".into(),
                reload: AdminConfigReloadResult {
                    applied: false,
                    error: Some(config_file_error()),
                    restart_needed: vec!["screen".into()],
                },
            },
        ),
        fixture(
            "admin_project_remove_result.json",
            &AdminProjectRemoveResult {
                project: project_info(),
                file: "/home/me/.config/efr/projects.toml".into(),
                reload: AdminConfigReloadResult {
                    applied: true,
                    error: None,
                    restart_needed: Vec::new(),
                },
            },
        ),
        fixture(
            "admin_status_result.json",
            &AdminStatusResult {
                daemon_id: daemon_id(),
                version: "0.1.0".into(),
                protocol: PROTOCOL_VERSION,
                pid: 1234,
                started_at: at("2026-10-03T08:00:00Z"),
                screen_backend: "vt100".into(),
                conversations: 2,
                shells: 1,
                providers: vec![ProviderStatus {
                    provider: "openai".into(),
                    logged_in: true,
                    expires_at: Some(at("2026-10-03T20:00:00Z")),
                }],
                roots: Some(daemon_roots()),
                config: Some(ConfigStatus {
                    path: "/home/me/.config/efr/config.toml".into(),
                    exists: true,
                    symlink_target: Some("/home/me/dotfiles/efr/config.toml".into()),
                    reload_error: Some(config_file_error()),
                    restart_needed: vec!["screen".into()],
                }),
                sandbox: Some(sandbox_status()),
                sandbox_paths: Some(SandboxPaths {
                    launcher: Some("/run/user/1000/efr/bin/efr-sbx".into()),
                    launcher_source: Some("/home/me/.local/lib/efr/efr-sbx".into()),
                    launcher_sha256_ok: Some(true),
                    state: "/home/me/.local/state/efr/sandbox".into(),
                    runtime: "/run/user/1000/efr/sbx".into(),
                }),
            },
        ),
        fixture(
            "admin_sandbox_check_result.json",
            &AdminSandboxCheckResult {
                status: SandboxStatus {
                    available: false,
                    reason: Some("bubblewrap is setuid; efr needs the unprivileged build".into()),
                    fix: Some("install the non-setuid build of bubblewrap".into()),
                    ..sandbox_status()
                },
                checks: vec![
                    SandboxCheck {
                        name: "landlock".into(),
                        outcome: CheckOutcome::Ok,
                        detail: Some("Landlock ABI 10, errata 0xf".into()),
                        fix: None,
                    },
                    SandboxCheck {
                        name: "bwrap".into(),
                        outcome: CheckOutcome::Fail,
                        detail: Some(
                            "bubblewrap is setuid; efr needs the unprivileged build".into(),
                        ),
                        fix: Some("install the non-setuid build of bubblewrap".into()),
                    },
                    SandboxCheck {
                        name: "path".into(),
                        outcome: CheckOutcome::Warn,
                        detail: Some("~/dotfiles/bin is on PATH and inside a project".into()),
                        fix: None,
                    },
                    SandboxCheck {
                        name: "self_test".into(),
                        outcome: CheckOutcome::Skipped,
                        detail: None,
                        fix: None,
                    },
                ],
                launch_us: Some(3600),
                snapshot_launch_us: Some(14_800),
            },
        ),
        fixture(
            "sandbox_explain_result.json",
            &SandboxExplainResult {
                path: "/home/me/.zshrc".into(),
                project: Some("/home/me/p/app".into()),
                mode: Mode::Auto,
                role: SandboxPathRole::Floor,
                read: true,
                write: false,
                reason: "a shell startup file (floor)".into(),
                write_exit: Some(ExitKind::Persistence),
            },
        ),
        fixture(
            "conversation_diff_result.json",
            &ConversationDiffResult {
                turn_id: turn_id(),
                changes: file_changes(),
                diff: Some(
                    "diff --git a/src/a.rs b/src/a.rs\nindex 3b18e51..a042389 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n"
                        .into(),
                ),
            },
        ),
        fixture(
            "sandbox_surface_respond_result.json",
            &SandboxSurfaceRespondResult { seq: Seq::new(47) },
        ),
        fixture(
            "admin_config_reload_result.json",
            &AdminConfigReloadResult {
                applied: false,
                error: Some(config_file_error()),
                restart_needed: vec!["screen".into()],
            },
        ),
        fixture(
            "admin_login_openai_item_authorize_url.json",
            &AdminLoginOpenAiItem::AuthorizeUrl {
                url: "https://auth.openai.com/oauth/authorize?response_type=code&state=st4te"
                    .into(),
            },
        ),
        fixture(
            "admin_login_openai_item_completed.json",
            &AdminLoginOpenAiItem::Completed { provider: "openai".into() },
        ),
    ]
}

/// A registered project with every member set.
fn project_info() -> ProjectInfo {
    ProjectInfo { id: project_id(), root: "/home/me/p/app".into(), name: Some("app".into()) }
}

/// A second project id, for a list of two.
fn other_project_id() -> ProjectId {
    parse("019a9b1c-3d00-7a10-8b20-00000000000e")
}

/// A built-in default model with efforts and a model from the config, so the result
/// freezes every member and every [`ModelSource`].
pub(crate) fn models_list_sample() -> ModelsListResult {
    ModelsListResult {
        models: vec![
            ModelInfo {
                id: "gpt-5.5".into(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
                default_effort: Some("medium".into()),
                default: true,
                source: ModelSource::Builtin,
                context_window: Some(272_000),
            },
            ModelInfo {
                id: "gpt-5.5-preview".into(),
                efforts: Vec::new(),
                default_effort: None,
                default: false,
                source: ModelSource::Config,
                context_window: None,
            },
        ],
    }
}

/// One sample per event kind, and one of a kind from the future.
pub(crate) fn event_samples() -> Vec<Event> {
    let mut future = Map::new();
    future.insert("device_id".into(), json!(device_id()));
    future.insert("label".into(), json!("phone"));
    vec![
        Event::ConversationCreated { origin: Origin::Shell, tty: Some("/dev/pts/3".into()) },
        Event::PromptQueued {
            turn_id: turn_id(),
            command_id: command_id(),
            text: "why is the disk full".into(),
            origin: Origin::Shell,
            context: Some(shell_context()),
            settings: turn_settings(),
            steers: vec![Seq::new(45)],
        },
        Event::PromptWithdrawn { turn_id: queued_turn_id(), origin: Origin::Shell },
        Event::PromptHeld { turn_id: turn_id() },
        Event::TurnStarted {
            turn_id: turn_id(),
            cwd: "/var/log".into(),
            scope: Scope::Path("/var/log".into()),
            settings: Some(fallen_back_settings()),
        },
        Event::ScopeChanged {
            turn_id: turn_id(),
            from: Scope::Machine,
            to: Scope::Project(project_id()),
        },
        message_updated(),
        Event::AssistantMessageCompleted {
            turn_id: turn_id(),
            index: 0,
            text: "The journal takes 3.1 GiB. `journalctl --vacuum-size=500M` frees most of it."
                .into(),
        },
        Event::ToolCallStarted {
            turn_id: turn_id(),
            call_id: call_id(),
            tool: "shell".into(),
            input: json!({ "command": "du -sh /var/log/*", "timeout_secs": 30 }),
            manual_input: true,
            launch: Some(Launch::Contained {
                grants: vec![Grant::Write { path: "/home/me/Documents".into() }],
            }), freeform: false,
        },
        Event::ToolCallOutputUpdated {
            turn_id: turn_id(),
            call_id: call_id(),
            tail: "3.1G\t/var/log/journal\n".into(),
            bytes: 4096,
        },
        Event::ToolCallInputChanged {
            turn_id: turn_id(),
            call_id: call_id(),
            input: InputWait::Visible,
            looks_secret: true,
        },
        Event::ToolCallCompleted {
            turn_id: turn_id(),
            call_id: call_id(),
            output: "3.1G\t/var/log/journal\n12K\t/var/log/pacman.log\n".into(),
            truncated: false,
            is_error: false,
            exit_code: Some(0),
            sandbox: Some(SandboxSummary {
                confined: true,
                cwd_changed: true,
                promoted: vec!["RUST_LOG".into()],
                kept_out: vec!["VIRTUAL_ENV".into()],
                dropped: vec!["LD_PRELOAD".into()],
                background_stopped: vec!["vite".into()],
                survivors: vec!["sudo".into()],
                blocked: vec![Blocked {
                    host: "example.com".into(),
                    port: 443,
                    reason: BlockReason::NotAllowed,
                }],
                surface_changes: vec![surface_change()],
                setup_error: Some("bwrap: Can't mount proc on /newroot/proc".into()),
            }),
            refusal: Some("efr's config (floor)".into()),
            changes: Some(file_changes()),
            diff: Some(
                "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    run();\n+    run_all();\n... 12 more lines\n"
                    .into(),
            ),
        },
        Event::ApprovalRequested {
            turn_id: turn_id(),
            call_id: call_id(),
            summary: "run `journalctl --vacuum-size=500M` as root".into(),
            diff_preview: Some("--- a/etc/systemd/journald.conf\n+++ b/etc/systemd/journald.conf\n-#SystemMaxUse=\n+SystemMaxUse=500M\n".into()),
            interactive: true,
            exit: Some(ExitInfo {
                kinds: vec![ExitKind::Write],
                launch: Launch::Contained {
                    grants: vec![Grant::Write { path: "/home/me/Documents".into() }],
                },
                grants: vec![Grant::Write { path: "/home/me/Documents".into() }],
                facts: vec!["~/Documents exists and is a directory".into()],
                model_reason: Some("the user asked for the report in Documents".into()),
                judged: Some(judgement()),
                user_only: true,
            }),
        },
        Event::ApprovalResolved {
            turn_id: turn_id(),
            call_id: call_id(),
            decision: ApprovalDecision::Deny,
            origin: Origin::Phone,
        },
        Event::ApprovalExpired { turn_id: turn_id(), call_id: call_id() },
        Event::TurnSteered { turn_id: turn_id(), text: "check the journal too".into() },
        Event::SteeringDelivered { turn_id: turn_id(), steers: vec![Seq::new(45), Seq::new(46)] },
        Event::SteeringWithdrawn {
            turn_id: turn_id(),
            steers: vec![Seq::new(46)],
            origin: Origin::Shell,
        },
        Event::TurnInterruptRequested { turn_id: turn_id(), origin: Origin::Cli },
        Event::TurnInterrupted {
            turn_id: turn_id(),
            usage: Some(Usage::new(900, 20)),
            context: Some(context_use()),
        },
        Event::TurnCompleted {
            turn_id: turn_id(),
            usage: Some(Usage {
                input_tokens: 1200,
                output_tokens: 340,
                cached_input_tokens: 1024,
                reasoning_tokens: 128,
                context_tokens: 89_000,
            }),
            context: Some(context_use()),
            changes: Some(file_changes()),
        },
        Event::TurnFailed {
            turn_id: turn_id(),
            error: ErrorBody::new(ErrorCode::Internal, "the provider stream ended early")
                .with_data(json!({ "provider": "openai", "status": 502 })),
            usage: Some(Usage::new(600, 0)),
            context: Some(context_use()),
        },
        Event::TurnCancelled { turn_id: turn_id() },
        Event::ShellStarted { pty_id: pty_id(), cwd: "/home/me".into(), pid: Some(6060) },
        Event::ShellExited { pty_id: pty_id(), exit_code: Some(0) },
        Event::CwdChanged { pty_id: pty_id(), cwd: "/var/log".into(), host: Some("desk".into()) },
        Event::LoginCompleted { provider: "openai".into() },
        Event::ExitRequested {
            turn_id: turn_id(),
            call_id: call_id(),
            kinds: vec![ExitKind::Write],
            grants: vec![Grant::Write { path: "/home/me/Documents".into() }],
            source: ExitSource::Needs,
            record: Box::new(exit_record()),
        },
        Event::ExitJudged {
            turn_id: turn_id(),
            call_id: call_id(),
            judge: JudgeKind::Classifier,
            verdict: Verdict::Deny,
            model: Some("codex-auto-review".into()),
            latency_ms: Some(1840),
            risk: Some(Risk::Critical),
            user_authorization: Some(UserAuthorization::None),
            category: Some("data exfiltration".into()),
            rationale: Some("no user message names the host".into()),
            record_sha256: Some(
                "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
            ),
            cached: true,
        },
        Event::SandboxSurfaceChanged {
            turn_id: turn_id(),
            call_id: call_id(),
            changes: vec![surface_change()],
            quarantined: true,
        },
        Event::SurfaceQuestionRequested {
            turn_id: turn_id(),
            call_id: call_id(),
            question_id: question_id(),
            changes: vec![surface_change()],
        },
        Event::SurfaceQuestionAnswered {
            turn_id: turn_id(),
            question_id: question_id(),
            keep: true,
            origin: Some(Origin::Shell),
        },
        Event::TurnSurfaceReport {
            turn_id: turn_id(),
            files: vec![
                ReportedFile {
                    path: ".cargo/config.toml".into(),
                    detail: Some("build.rustc-wrapper".into()),
                },
                ReportedFile { path: "build.rs".into(), detail: None },
            ],
        },
        Event::SandboxUnavailable {
            reason: "Landlock ABI 6 found; auto needs 9 (Linux 7.1)".into(),
        },
        Event::ConversationCompacted(compaction()),
        Event::Unknown { kind: "device_enrolled".into(), payload: future },
    ]
}

/// Every kind of input wait, so `input_waits.json` freezes the wire form of each.
pub(crate) fn input_wait_samples() -> Vec<InputWait> {
    vec![InputWait::None, InputWait::Visible, InputWait::Hidden]
}

/// One sample of every frame shape.
fn frame_fixtures() -> Vec<Fixture> {
    let id = RequestId::new(7);
    let item = serde_json::to_value(LeaseReportResult { ttl_secs: 45 })
        .unwrap_or_else(|err| panic!("the item sample does not encode: {err}"));
    vec![
        fixture(
            "frames/client_request.json",
            &ClientFrame::Request {
                id,
                method: Method::ConversationsList(ConversationsList::default()),
            },
        ),
        fixture("frames/client_cancel.json", &ClientFrame::Cancel { id }),
        fixture("frames/server_item.json", &ServerFrame::Item { id, item }),
        fixture("frames/server_end.json", &ServerFrame::end(id)),
        fixture(
            "frames/server_error.json",
            &ServerFrame::error(Some(id), ErrorBody::overflow(Seq::new(41))),
        ),
        fixture(
            "frames/server_error_without_id.json",
            &ServerFrame::error(None, ErrorBody::new(ErrorCode::Invalid, "the frame is not JSON")),
        ),
        fixture("frames/server_ack.json", &ServerFrame::Ack { id }),
    ]
}

#[cfg(test)]
mod tests;
