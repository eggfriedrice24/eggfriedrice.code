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
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};

use crate::{
    ActionFacts, AdminConfigReload, AdminConfigReloadResult, AdminLoginApiKey,
    AdminLoginApiKeyResult, AdminLoginOpenAi, AdminLoginOpenAiItem, AdminLogout, AdminLogoutResult,
    AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove, AdminProjectRemoveResult,
    AdminSandboxCheck, AdminSandboxCheckResult, AdminStatus, AdminStatusResult, AlwaysSaved,
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, ApprovedBy, AttachHow, Base64Bytes,
    BlockReason, Blocked, BusKind, BusProxyOff, BusRefusal, BusyShell, CacheMode, CacheSummary,
    CallBackground, CallBackgroundResult, CallId, Capabilities, CatalogOrigin, CatalogStatus, Cell,
    ChainStep, ChangeKind, CheckOutcome, ClientFrame, Color, CommandId, Compaction,
    CompactionBrief, CompactionId, CompactionSummary, CompactionTrigger, ConfigFileError,
    ConfigStatus, ContextItem, ContextPart, ContextPartKind, ContextUse, ConversationCompact,
    ConversationCompactResult, ConversationContext, ConversationContextResult, ConversationDelete,
    ConversationDeleteResult, ConversationDiff, ConversationDiffResult, ConversationHistory,
    ConversationHistoryResult, ConversationId, ConversationResume, ConversationResumeResult,
    ConversationSearch, ConversationSearchResult, ConversationSnapshot, ConversationStatus,
    ConversationSubscribe, ConversationSubscribeItem, ConversationSummary, ConversationsList,
    ConversationsListResult, ConversationsRecent, ConversationsRecentResult, ConversationsSearch,
    ConversationsSearchResult, ConversationsWatch, ConversationsWatchItem, CountedUse, Cursor,
    DaemonId, DaemonPaths, DaemonRoots, DesktopEnv, DesktopKind, DeviceEnroll, DeviceEnrollResult,
    DeviceId, DeviceInfo, DeviceList, DeviceListResult, DevicePairConfirm, DevicePairConfirmResult,
    DevicePairStart, DevicePairStartItem, DeviceProve, DeviceProveResult, DeviceRevoke,
    DeviceRevokeResult, Draft, DraftPart, EffectiveSettings, ErrorBody, ErrorCode, Event,
    EventEnvelope, ExitFacts, ExitInfo, ExitKind, ExitRecord, ExitSource, FileChange, FileChanges,
    ForwardOutcome, FullModeStatus, FullRefusal, GitCodeFact, GitCodeFacts, GitCounts, Grant,
    HeldConfig, HeldReason, Hello, HelloResult, HostFact, InputRespond, InputRespondResult,
    InputWait, JobCgroup, JobId, JobInfo, JobState, JobStopBy, JobsList, JobsListResult, JobsStop,
    JobsStopResult, JudgeKind, Judgement, LateSteer, Launch, LeaseReport, LeaseReportResult,
    LiveTurn, LoginKind, Method, Mode, ModeFallback, ModelInfo, ModelSource, ModelsList,
    ModelsListResult, MovedCall, NetworkMode, NewReason, NotSavedReason, NoticeKind, Origin,
    OverriddenSettings, PROTOCOL_VERSION, PageCursor, PairEnd, PairingId, PairingQr, PathClassName,
    PauseReason, PortForward, Presence, PresenceSet, PresenceSetResult, PresenceSource,
    ProgramFact, ProjectId, ProjectInfo, ProjectsList, ProjectsListResult, PromptSend,
    PromptSendResult, PromptWithdraw, PromptWithdrawResult, ProveContext, ProveMessage,
    ProviderStatus, PtyAttach, PtyAttachItem, PtyId, PtyResize, PtyResizeResult, PtyWrite,
    PtyWriteResult, QuestionId, RecentCommand, RecentConversation, RefChange, RepoHint,
    ReportedFile, RequestId, ResentSteers, Risk, RootDir, RootFiles, RootSource, RouteLabel,
    RouteScope, Routed, RowCells, SandboxCheck, SandboxExplain, SandboxExplainResult,
    SandboxPathRole, SandboxPaths, SandboxStatus, SandboxSummary, SandboxSurfaceRespond,
    SandboxSurfaceRespondResult, Scope, ScopeName, ScreenSnapshot, SearchHit, SearchIndexState,
    SearchKind, SearchLine, SearchPartial, SearchRow, SearchStats, SecretText, Seq, ServerFrame,
    ShellCommandEnd, ShellContext, ShellInterrupt, ShellInterruptResult, ShellStatus,
    ShellStatusResult, Size, SnapshotPhase, StopReason, SurfaceChange, Switch, TargetFact,
    TextRange, TurnId, TurnInterrupt, TurnInterruptResult, TurnSettings, TurnSteer,
    TurnSteerResult, TurnStop, TurnUndo, TurnUndoInfo, TurnUndoResult, UndoAction, UndoEntry,
    UndoId, UndoReason, UndoRefused, UndoableTurn, Usage, UserAuthorization, Verdict, Watch,
    WithdrawTarget, WithdrawnPrompt, WithdrawnSteer,
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
    fixtures.push(fixture("events/exit_requested_floor_edit.json", &floor_edit_exit_sample()));
    fixtures.push(fixture("events/exit_judged_full.json", &full_judged_sample()));
    fixtures.push(fixture("events/job_started_moved.json", &moved_job_sample()));
    fixtures
        .push(fixture("events/approval_requested_local_only.json", &local_only_approval_sample()));
    fixtures.push(fixture("events/approval_requested_reason.json", &reason_approval_sample()));
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
    fixtures.push(fixture("undo_actions.json", &UndoAction::ALL.to_vec()));
    fixtures.push(fixture("undo_reasons.json", &UndoReason::ALL.to_vec()));
    fixtures.push(fixture("undo_refusals.json", &undo_refused_samples()));
    fixtures.push(fixture("job_states.json", &JobState::ALL.to_vec()));
    fixtures.push(fixture("job_stop_causes.json", &JobStopBy::ALL.to_vec()));
    fixtures.push(fixture("held_reasons.json", &HeldReason::ALL.to_vec()));
    fixtures.push(fixture("notice_kinds.json", &NoticeKind::ALL.to_vec()));
    fixtures.push(fixture("forward_outcomes.json", &ForwardOutcome::ALL.to_vec()));
    fixtures.push(fixture("block_reasons.json", &BlockReason::ALL.to_vec()));
    fixtures.push(fixture("approval_decisions.json", &approval_decision_samples()));
    fixtures.push(fixture("always_saved.json", &always_saved_samples()));
    fixtures.push(fixture("not_saved_reasons.json", &NotSavedReason::ALL.to_vec()));
    fixtures.push(fixture("bus_proxy_off_reasons.json", &BusProxyOff::ALL.to_vec()));
    fixtures.push(fixture("judge_kinds.json", &JudgeKind::ALL.to_vec()));
    fixtures.push(fixture("full_refusals.json", &FullRefusal::ALL.to_vec()));
    fixtures.push(fixture("context_part_kinds.json", &ContextPartKind::ALL.to_vec()));
    fixtures.push(fixture("route_scopes.json", &route_scope_samples()));
    fixtures.push(fixture("route_switches.json", &switch_samples()));
    fixtures.push(fixture("attach_hows.json", &AttachHow::ALL.to_vec()));
    fixtures.push(fixture("shell_command_ends.json", &ShellCommandEnd::ALL.to_vec()));
    fixtures.push(fixture("approved_bys.json", &ApprovedBy::ALL.to_vec()));
    fixtures.push(fixture("search_kinds.json", &SearchKind::ALL.to_vec()));
    fixtures.push(fixture("search_partials.json", &SearchPartial::ALL.to_vec()));
    fixtures.push(fixture("pair_ends.json", &PairEnd::ALL.to_vec()));
    fixtures.push(fixture("device_prove_message.json", &prove_vector()));
    fixtures.push(fixture("pairing_qr_url.json", &qr_vector()));
    fixtures
}

/// Every form of a route scope key, so `route_scopes.json` freezes the wire form of
/// each.
pub(crate) fn route_scope_samples() -> Vec<RouteScope> {
    vec![RouteScope::Project(project_id()), RouteScope::Outside, RouteScope::Unknown]
}

/// Every switch of a terminal's conversation and every reason of a new one, so
/// `route_switches.json` freezes the wire form of each.
pub(crate) fn switch_samples() -> Vec<Switch> {
    let updated_at = at("2026-10-03T10:00:05Z");
    let home = RouteLabel { scope: RouteScope::Outside, name: None, dir: "/home/me".into() };
    vec![
        Switch::Back { title: Some("fix the tests".into()), updated_at },
        Switch::PickedUp { title: Some("fix the proxy test".into()), updated_at },
        Switch::New { from: Some(home), reason: NewReason::Scope },
        Switch::New { from: None, reason: NewReason::Idle { hours: 13 } },
        Switch::New {
            from: None,
            reason: NewReason::Moved {
                tty: "/dev/pts/7".into(),
                title: Some("fix the tests".into()),
            },
        },
        Switch::New { from: None, reason: NewReason::Unknown },
        Switch::Unknown,
    ]
}

/// Every answer to an approval request, so `approval_decisions.json` freezes the wire
/// form of each.
pub(crate) fn approval_decision_samples() -> Vec<ApprovalDecision> {
    vec![ApprovalDecision::Allow, ApprovalDecision::Deny, ApprovalDecision::AllowAlways]
}

/// Every state of an always rule's outcome, so `always_saved.json` freezes the wire
/// form of each.
pub(crate) fn always_saved_samples() -> Vec<AlwaysSaved> {
    vec![
        AlwaysSaved::Saved { index: 2, rule: "write under ~/notes".into() },
        AlwaysSaved::NotSaved { reason: NotSavedReason::ReadOnly },
        AlwaysSaved::Unknown,
    ]
}

/// Bus calls that the bus filter refused, with every member set.
fn bus_refused() -> Vec<BusRefusal> {
    vec![
        BusRefusal {
            bus: BusKind::System,
            name: "org.freedesktop.systemd1".into(),
            member: "org.freedesktop.systemd1.Manager.StartUnit".into(),
            path: "/org/freedesktop/systemd1".into(),
            count: 2,
        },
        BusRefusal {
            bus: BusKind::Session,
            name: "org.freedesktop.Notifications".into(),
            member: "org.freedesktop.Notifications.Notify".into(),
            path: "/org/freedesktop/Notifications".into(),
            count: 1,
        },
    ]
}

/// A job that ran, with every member set.
fn job_info() -> JobInfo {
    JobInfo {
        id: JobId::nth(3),
        conversation_id: conversation_id(),
        turn_id: turn_id(),
        call_id: call_id(),
        command: "npm run dev".into(),
        launch: Launch::Contained { grants: vec![Grant::OpenNetwork] },
        state: JobState::Stopped,
        started_at: at("2026-10-12T10:00:00Z"),
        ends_by: at("2026-10-12T11:00:00Z"),
        ended_at: Some(at("2026-10-12T10:32:00Z")),
        exit_code: Some(143),
        stopped_by: Some(JobStopBy::User),
        output: "/home/me/.local/share/efr/scratch/2026-10-12-019a9b1c/jobs/j3.out".into(),
        output_bytes: 48_213,
        watch: Some(Watch { pattern: "error|panic".into(), max_wakes: 3 }),
        wakes: 1,
        forwards: vec![
            PortForward { listen: "127.0.0.1".into(), port: 8765 },
            PortForward { listen: "::1".into(), port: 5173 },
        ],
        moved_from: None,
        waits_input: false,
        cgroup: Some(JobCgroup {
            off: None,
            memory_bytes: Some(183_500_800),
            cpu_usec: Some(42_100_000),
        }),
    }
}

/// A job that the user moved from a running `shell` call with Ctrl+B: a call outside the
/// sandbox, whose yes to the move covered the exit of the job decision.
fn moved_job_info() -> JobInfo {
    JobInfo {
        id: JobId::nth(4),
        command: "cargo test --workspace".into(),
        launch: Launch::Unsandboxed,
        state: JobState::Running,
        started_at: at("2026-10-12T10:05:42Z"),
        ends_by: at("2026-10-12T11:05:42Z"),
        ended_at: None,
        exit_code: None,
        stopped_by: None,
        output: "/home/me/.local/share/efr/scratch/2026-10-12-019a9b1c/jobs/j4.out".into(),
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: Some(MovedCall {
            call_id: call_id(),
            turn_id: turn_id(),
            after_secs: 42,
            exits: vec![ExitInfo {
                kinds: vec![ExitKind::Destructive],
                launch: Launch::Unsandboxed,
                grants: Vec::new(),
                facts: vec!["a delete that undo does not cover as a job".into()],
                model_reason: None,
                judged: None,
                user_only: false,
                always: None,
            }],
            launcher: true,
        }),
        cgroup: Some(JobCgroup {
            off: Some("efrd's cgroup is not delegated (Delegate=yes)".into()),
            memory_bytes: None,
            cpu_usec: None,
        }),
        ..job_info()
    }
}

/// The start of a job that the user moved from a running call: the hidden shell's zsh
/// stands in for the launcher.
pub(crate) fn moved_job_sample() -> Event {
    Event::JobStarted {
        turn_id: turn_id(),
        call_id: call_id(),
        job: moved_job_info(),
        launcher_pid: 48_190,
        launcher_start: 9_118_002,
    }
}

/// Every refusal of `turn.undo`, so `undo_refusals.json` freezes the wire form of each.
pub(crate) fn undo_refused_samples() -> Vec<UndoRefused> {
    vec![
        UndoRefused::Off,
        UndoRefused::NothingToUndo,
        UndoRefused::TurnNotFound,
        UndoRefused::LaterTurnsNotUndone { turn_id: queued_turn_id() },
        UndoRefused::NoRedo,
        UndoRefused::PhoneOrigin,
        UndoRefused::StoreFailed,
    ]
}

fn undo_id() -> UndoId {
    parse("019a9b1c-3d00-7a10-8b20-00000000000f")
}

/// What undo can take back of a turn, with every member set.
fn turn_undo_info() -> TurnUndoInfo {
    TurnUndoInfo {
        undoable: true,
        roots: vec![
            RootFiles { shown: "eggfriedrice.code".into(), files: 4, refs: 1 },
            RootFiles { shown: "$SCRATCH".into(), files: 1, refs: 0 },
        ],
        home_files: 2,
        not_covered: vec!["target/ (ignored, large)".into(), "refs of app (shallow)".into()],
        not_covered_more: 1,
    }
}

/// A ref that an undo moved, with every member set.
fn ref_change() -> RefChange {
    RefChange {
        root: "eggfriedrice.code".into(),
        name: "main".into(),
        from: Some("9fceb02".into()),
        to: Some("3b18e51".into()),
        applied: Some(true),
        reason: Some(UndoReason::ChangedAfterTurn),
    }
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
        fresh: Some(
            "<fresh-context>\n## Directories\n- The user's directory: /var\n</fresh-context>"
                .into(),
        ),
        summary: Some("## Task and state\nFree space on /var.".into()),
        usage: Some(Usage {
            input_tokens: 205_000,
            output_tokens: 3_200,
            cached_input_tokens: 198_000,
            reasoning_tokens: 900,
            cache_write_tokens: 6_000,
            cache_write_1h_tokens: 0,
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

/// The recent commands of a prompt: one with every member and one with none.
fn recent_commands() -> Vec<RecentCommand> {
    vec![
        RecentCommand {
            command: "journalctl --disk-usage".into(),
            status: Some(0),
            cwd: Some("/home/me".into()),
            ended_at: Some(at("2026-10-20T09:58:12Z")),
            duration_ms: Some(840),
        },
        RecentCommand::new("du -sh /var/log"),
    ]
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
        desktop: Some(Box::new(DesktopEnv {
            wayland_display: Some("wayland-1".into()),
            display: Some(":0".into()),
            hyprland_instance_signature: Some(
                "0b4a3e5a8d1c2f6e7a9b0c1d2e3f4a5b6c7d8e9f_1760000000_1234567890".into(),
            ),
            swaysock: Some("/run/user/1000/sway-ipc.1000.1234.sock".into()),
            i3sock: Some("/run/user/1000/i3/ipc-socket.1234".into()),
            niri_socket: Some("/run/user/1000/niri.wayland-1.1234.sock".into()),
            has_xauthority: true,
        })),
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
        provider: None,
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

/// One grant of every kind, and a desktop grant of every desktop, so `grants.json`
/// freezes the wire form of each.
pub(crate) fn grant_samples() -> Vec<Grant> {
    let mut grants = vec![
        Grant::Write { path: "/home/me/notes".into() },
        Grant::Host { host: "registry.npmjs.org".into(), port: 443 },
        Grant::OpenNetwork,
        Grant::Socket { path: "/run/user/1000/app.sock".into() },
        Grant::Bus { bus: BusKind::Session },
        Grant::Bus { bus: BusKind::System },
        Grant::Device { path: "/dev/nvme0n1".into() },
        Grant::Unmask { path: "/home/me/p/app/.env".into() },
        Grant::NetView,
        Grant::BusFilter { bus: BusKind::System },
        Grant::BusFilter { bus: BusKind::Session },
        Grant::Unknown,
    ];
    grants.extend(DesktopKind::ALL.map(|desktop| Grant::Desktop { desktop }));
    grants
}

/// Every launch, so `launches.json` freezes the wire form of each.
pub(crate) fn launch_samples() -> Vec<Launch> {
    vec![
        Launch::Direct,
        Launch::contained(),
        Launch::Contained { grants: vec![Grant::OpenNetwork] },
        Launch::Contained {
            grants: vec![Grant::NetView, Grant::Write { path: "/home/me/notes".into() }],
        },
        Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] },
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
        netview: true,
        netview_off: false,
        proxy: true,
        bus_proxy: true,
        bus_proxy_off: None,
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
            edit: None,
            background: true,
            max_minutes: Some(180),
            watch: true,
            wake_turn: true,
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
                backup_before_write: false,
                masked: false,
            }],
            hosts: vec![HostFact {
                host: "example.com".into(),
                on_allow_list: false,
                named_in_user_messages: false,
                refused_by_proxy: true,
                project_remote: false,
            }],
            programs: vec![ProgramFact {
                word: "cp".into(),
                resolved: Some("/usr/bin/cp".into()),
                in_write_root: false,
                changed_this_turn: false,
            }],
            upload_patterns: vec!["git push".into()],
            outside_commands: vec!["df -h /".into()],
            desktop_read: true,
            repo_surface_changed_this_turn: true,
            git_status: Some(GitCounts { modified: 3, untracked: 1, staged: 0 }),
            snapshot_covers: Some(false),
            sandbox_export_names: vec!["VIRTUAL_ENV".into()],
            previous_exits_this_turn: vec![(ExitKind::Host, Verdict::Allow)],
            refusals_in_a_row: 1,
            denials_in_a_row: 2,
            denials_this_turn: 4,
            git_code: Some(GitCodeFacts {
                checked: true,
                items: vec![GitCodeFact {
                    repo: "/home/me/.local/share/efr/scratch/c/r".into(),
                    item: "hooks/pre-push".into(),
                    file: "/home/me/.local/share/efr/scratch/c/r/.git/hooks/pre-push".into(),
                    writable: true,
                }],
            }),
        },
    }
}

/// The record of a `write_file` call that edits a floor file: it carries the payload and
/// the backup fact, which a shell line never has.
fn floor_edit_record() -> ExitRecord {
    ExitRecord {
        version: ExitRecord::VERSION,
        user_messages: vec!["add alias k=kubectl to my zshrc".into()],
        action: ActionFacts {
            tool: "write_file".into(),
            line: String::new(),
            cwd: "/home/me/p/app".into(),
            scope: Scope::Project(project_id()),
            exits: vec![ExitKind::Persistence],
            grants: Vec::new(),
            source: ExitSource::Predicted,
            edit: Some("export EDITOR=nvim\nalias k=kubectl\n".into()),
            background: false,
            max_minutes: None,
            watch: false,
            wake_turn: false,
        },
        facts: ExitFacts {
            targets: vec![TargetFact {
                path: "/home/me/.zshrc".into(),
                class: Some(PathClassName::UserConfig),
                in_write_root: false,
                floor: true,
                synced: false,
                exists: true,
                named_in_user_messages: true,
                backup_before_write: true,
                masked: false,
            }],
            ..ExitFacts::default()
        },
    }
}

/// An `exit_judged` of an exit that full mode let through: no model, no review. The
/// sample of `events/exit_judged.json` is the classifier's, with every member set.
pub(crate) fn full_judged_sample() -> Event {
    Event::ExitJudged {
        turn_id: turn_id(),
        call_id: call_id(),
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
    }
}

/// An `exit_requested` of a floor file edit through a file tool. The sample of
/// `events/exit_requested.json` is a shell line, which has no payload.
pub(crate) fn floor_edit_exit_sample() -> Event {
    Event::ExitRequested {
        turn_id: turn_id(),
        call_id: call_id(),
        kinds: vec![ExitKind::Persistence],
        grants: Vec::new(),
        source: ExitSource::Predicted,
        record: Box::new(floor_edit_record()),
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
                search: None,
                drafts_coalesced: None,
                snapshot_live: None,
                approval_local_only: None,
                device_prove: None,
                conversations_watch: None,
                extra: Default::default(),
            },
            tty: Some("/dev/pts/3".into()),
            pid: Some(5150),
            device_id: Some(device_id()),
        }),
        Method::ConversationsList(ConversationsList {
            cursor: Some(PageCursor::new("c1:019a9b1c-3d00-7a10-8b20-000000000001")),
            limit: Some(20),
            tty: Some("/dev/pts/3".into()),
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
            recent_commands: recent_commands(),
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
                recent_commands: recent_commands(),
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
                recent_commands: recent_commands(),
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
        Method::AdminLoginApiKey(AdminLoginApiKey {
            provider: "anthropic-api".into(),
            key: SecretText::new("sk-ant-api03-efr-fixture-a1b2"),
            check: false,
        }),
        Method::AdminLogout(AdminLogout { provider: "anthropic-api".into() }),
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
        Method::PresenceSet(PresenceSet {
            command_id: command_id(),
            conversation_id: Some(conversation_id()),
            presence: Presence::Away,
        }),
        Method::TurnUndo(TurnUndo {
            command_id: command_id(),
            conversation_id: Some(conversation_id()),
            turn_id: Some(turn_id()),
            dry_run: true,
            force: true,
            redo: true,
            list: true,
        }),
        Method::JobsList(JobsList { conversation_id: Some(conversation_id()) }),
        Method::JobsStop(JobsStop {
            command_id: command_id(),
            conversation_id: Some(conversation_id()),
            job_id: JobId::nth(3),
        }),
        Method::ConversationContext(ConversationContext {
            conversation_id: conversation_id(),
            model: Some("claude-opus-5-5".into()),
        }),
        Method::ConversationsRecent(ConversationsRecent {
            tty: Some("/dev/pts/3".into()),
            pwd: Some("/home/me/p/app/src".into()),
            limit: Some(10),
        }),
        Method::ConversationResume(ConversationResume {
            command_id: command_id(),
            conversation_id: conversation_id(),
            context: shell_context(),
            take: true,
        }),
        Method::ShellInterrupt(ShellInterrupt {
            conversation_id: conversation_id(),
            call_id: call_id(),
            command_id: command_id(),
            close: true,
        }),
        Method::ShellStatus(ShellStatus { conversation_id: conversation_id() }),
        Method::CallBackground(CallBackground {
            conversation_id: conversation_id(),
            call_id: call_id(),
            command_id: command_id(),
            confirm: Some(7_310_442_918_005),
        }),
        Method::ConversationsSearch(ConversationsSearch {
            query: "disk full".into(),
            tty: Some("/dev/pts/3".into()),
            pwd: Some("/home/me/p/app".into()),
            limit: Some(50),
            stats: true,
        }),
        Method::ConversationSearch(ConversationSearch {
            conversation_id: conversation_id(),
            query: "disk full".into(),
            limit: Some(20),
        }),
        Method::ConversationDelete(ConversationDelete {
            command_id: command_id(),
            conversation_id: conversation_id(),
            force: true,
            stop: true,
        }),
        Method::DevicePairStart(DevicePairStart { name: Some("Anna's iPhone".into()) }),
        Method::DevicePairConfirm(DevicePairConfirm { pairing_id: pairing_id(), accept: true }),
        Method::DeviceEnroll(DeviceEnroll {
            token: SecretText::new(PAIRING_TOKEN),
            name: "Anna's iPhone".into(),
            public_key: Base64Bytes::new(public_key()),
        }),
        Method::DeviceProve(DeviceProve {
            device_id: device_id(),
            signature: Base64Bytes::new(signature()),
        }),
        Method::DeviceList(DeviceList {}),
        Method::DeviceRevoke(DeviceRevoke { device_id: device_id() }),
        Method::ConversationsWatch(ConversationsWatch { limit: Some(20) }),
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
                    search: Some(true),
                    drafts_coalesced: None,
                    snapshot_live: None,
                    approval_local_only: None,
                    device_prove: None,
                    conversations_watch: None,
                    extra: Default::default(),
                },
                paths: DaemonPaths {
                    scratch_root: "/home/me/.local/share/efr/scratch".into(),
                    data_dir: "/home/me/.local/share/efr".into(),
                },
                challenge: "3q2-7wAAAAAAAAAAAAAAAA".into(),
                prove: None,
            },
        ),
        fixture("hello_result_remote.json", &remote_hello_result()),
        fixture("hello_result_remote_watch.json", &watch_hello_result()),
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
                live: None,
            }),
        ),
        fixture(
            "conversation_subscribe_item_snapshot_live.json",
            &ConversationSubscribeItem::Snapshot(ConversationSnapshot {
                conversation: summary(),
                events: vec![envelope(42, message_updated())],
                history_cursor: Some(PageCursor::new("h1:42")),
                hwm: Seq::new(42),
                live: Some(live_turn()),
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
                        stop: None,
                        undo: None,
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
                route: Some(Box::new(Routed {
                    scope: RouteLabel {
                        scope: RouteScope::Project(project_id()),
                        name: Some("app".into()),
                        dir: "/home/me/p/app".into(),
                    },
                    switch: Some(Switch::Back {
                        title: Some("fix the tests".into()),
                        updated_at: at("2026-10-03T10:00:05Z"),
                    }),
                    hint: Some(RepoHint { root: "/home/me/p/tool".into() }),
                })),
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
                    warnings: Vec::new(),
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
                    warnings: Vec::new(),
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
                providers: vec![
                    ProviderStatus {
                        provider: "openai-subscription".into(),
                        logged_in: true,
                        expires_at: Some(at("2026-10-03T20:00:00Z")),
                        active: false,
                        login: Some(LoginKind::Subscription),
                        key_hint: None,
                        key_refused_at: None,
                        key_checked_at: None,
                    },
                    ProviderStatus {
                        provider: "anthropic-api".into(),
                        logged_in: true,
                        expires_at: None,
                        active: true,
                        login: Some(LoginKind::ApiKey),
                        key_hint: Some("sk-ant-...a1b2".into()),
                        key_refused_at: Some(at("2026-10-03T07:45:00Z")),
                        key_checked_at: Some(at("2026-10-03T07:45:00Z")),
                    },
                ],
                catalog: Some(CatalogStatus {
                    provider: Some("anthropic-api".into()),
                    origin: CatalogOrigin::Backend,
                    fetched_at: Some(at("2026-10-03T07:30:00Z")),
                }),
                roots: Some(daemon_roots()),
                config: Some(ConfigStatus {
                    path: "/home/me/.config/efr/config.toml".into(),
                    exists: true,
                    symlink_target: Some("/home/me/dotfiles/efr/config.toml".into()),
                    reload_error: Some(config_file_error()),
                    restart_needed: vec!["screen".into()],
                    held: Some(HeldConfig {
                        loosens: vec!["permissions.rules".into(), "permissions.allow_full".into()],
                    }),
                }),
                sandbox: Some(sandbox_status()),
                sandbox_paths: Some(SandboxPaths {
                    launcher: Some("/run/user/1000/efr/bin/efr-sbx".into()),
                    launcher_source: Some("/home/me/.local/lib/efr/efr-sbx".into()),
                    launcher_sha256_ok: Some(true),
                    state: "/home/me/.local/state/efr/sandbox".into(),
                    runtime: "/run/user/1000/efr/sbx".into(),
                }),
                full_mode: Some(FullModeStatus {
                    allowed: true,
                    refused: Some(FullRefusal::SnapshotOff),
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
                    netview: false,
                    proxy: false,
                    bus_proxy: false,
                    bus_proxy_off: Some(BusProxyOff::NoProgram),
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
            "turn_undo_result.json",
            &TurnUndoResult {
                conversation_id: conversation_id(),
                turn_id: Some(turn_id()),
                title: Some("clean up the journal".into()),
                undo_id: Some(undo_id()),
                applied: true,
                stopped: false,
                resumed: true,
                entries: vec![
                    UndoEntry { action: UndoAction::Restore, path: "src/a.rs".into(), reason: None },
                    UndoEntry { action: UndoAction::Trash, path: "notes.md".into(), reason: None },
                    UndoEntry {
                        action: UndoAction::Skip,
                        path: "~/.zshrc".into(),
                        reason: Some(UndoReason::ChangedAfterTurn),
                    },
                    UndoEntry {
                        action: UndoAction::NotCovered,
                        path: ".env.large".into(),
                        reason: Some(UndoReason::LeftOut),
                    },
                    UndoEntry { action: UndoAction::Command, path: "@crontab".into(), reason: None },
                ],
                entries_more: 3,
                refs: vec![ref_change()],
                trash: Some(
                    "/home/me/.local/share/efr/undo-trash/u-019a9b1c-3d00-7a10-8b20-00000000000f"
                        .into(),
                ),
                notes: vec!["your git index stays as it is".into()],
                turns: vec![UndoableTurn {
                    turn_id: turn_id(),
                    title: Some("clean up the journal".into()),
                    auto: true,
                    full: true,
                    undone: true,
                    stopped: true,
                    ended: Some(at("2026-10-11T09:30:00Z")),
                }],
            },
        ),
        fixture(
            "presence_set_result.json",
            &PresenceSetResult {
                presence: Presence::Away,
                changed: vec![conversation_id()],
                seq: Some(Seq::new(52)),
            },
        ),
        fixture(
            "admin_config_reload_result.json",
            &AdminConfigReloadResult {
                applied: false,
                error: Some(config_file_error()),
                restart_needed: vec!["screen".into()],
                warnings: vec![
                    "sandbox.allow[0] is disabled: it names a git dir; it covers no call".into(),
                ],
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
        fixture(
            "admin_login_api_key_result.json",
            &AdminLoginApiKeyResult {
                provider: "anthropic-api".into(),
                key_hint: "sk-ant-...a1b2".into(),
                checked: true,
                active: false,
            },
        ),
        fixture(
            "admin_logout_result.json",
            &AdminLogoutResult {
                provider: "anthropic-api".into(),
                logged_out: true,
                active: true,
            },
        ),
        fixture(
            "jobs_list_result.json",
            &JobsListResult {
                jobs: vec![
                    job_info(),
                    JobInfo { waits_input: true, ..moved_job_info() },
                ],
            },
        ),
        fixture("jobs_stop_result.json", &JobsStopResult { job: job_info() }),
        fixture("conversation_context_result.json", &context_report()),
        fixture("conversations_recent_result.json", &recent_list()),
        fixture(
            "conversations_watch_item_snapshot.json",
            &ConversationsWatchItem::Snapshot { conversations: vec![watch_row()] },
        ),
        fixture(
            "conversations_watch_item_changed.json",
            &ConversationsWatchItem::Changed { conversation: Box::new(waiting_watch_row()) },
        ),
        fixture(
            "conversations_watch_item_removed.json",
            &ConversationsWatchItem::Removed { conversation_id: conversation_id() },
        ),
        fixture(
            "conversation_resume_result.json",
            &ConversationResumeResult::Resumed {
                conversation_id: conversation_id(),
                scope: app_label(),
                title: Some("fix the tests".into()),
                updated_at: at("2026-10-03T10:00:05Z"),
                shell_dir: Some("/home/me/p/app/src".into()),
                here: true,
                continues_here: true,
            },
        ),
        fixture("shell_interrupt_result.json", &ShellInterruptResult {}),
        fixture(
            "shell_status_result.json",
            &ShellStatusResult {
                busy: Some(BusyShell {
                    call_id: call_id(),
                    command: "rsync -a -e ssh me@build:.cache/out ./out".into(),
                    since: at("2026-10-03T10:00:01Z"),
                    contained: false,
                    input: InputWait::Hidden,
                    looks_secret: true,
                    prompt: "me@build's password: ".into(),
                    approved_by: Some(ApprovedBy::YourRule),
                }),
            },
        ),
        fixture(
            "call_background_result.json",
            &CallBackgroundResult::Moved { job_id: JobId::nth(4) },
        ),
        fixture(
            "call_background_result_preview.json",
            &CallBackgroundResult::Preview {
                token: 7_310_442_918_005,
                command: "cargo test --workspace".into(),
                max_minutes: 60,
                exits: moved_job_info().moved_from.map(|moved| moved.exits).unwrap_or_default(),
            },
        ),
        fixture("conversations_search_result.json", &search_list()),
        fixture("conversation_search_result.json", &search_in_one()),
        fixture(
            "conversation_resume_result_held.json",
            &ConversationResumeResult::Held {
                tty: "/dev/pts/4".into(),
                title: Some("fix the tests".into()),
            },
        ),
        fixture(
            "conversation_delete_result.json",
            &ConversationDeleteResult::Deleted {
                conversation_id: conversation_id(),
                title: Some("fix the tests".into()),
                jobs_stopped: 2,
                not_removed: vec!["/home/me/.local/share/efr/scratch/2026-10-03-fix-a1b2c3d4".into()],
            },
        ),
        fixture(
            "conversation_delete_result_held.json",
            &ConversationDeleteResult::Held {
                tty: "/dev/pts/4".into(),
                title: Some("fix the tests".into()),
            },
        ),
        fixture(
            "device_pair_start_item_started.json",
            &DevicePairStartItem::Started {
                pairing_id: pairing_id(),
                expires_at: at("2026-10-26T09:05:00Z"),
                qr: pairing_qr(),
            },
        ),
        fixture(
            "device_pair_start_item_pending.json",
            &DevicePairStartItem::Pending {
                pairing_id: pairing_id(),
                device_name: "Anna's iPhone".into(),
                node_name: "annas-iphone".into(),
                user_login: "anna@example.com".into(),
                key_fingerprint: KEY_FINGERPRINT.into(),
                os: Some("iOS".into()),
            },
        ),
        fixture(
            "device_pair_start_item_paired.json",
            &DevicePairStartItem::Paired { pairing_id: pairing_id(), device_id: device_id() },
        ),
        fixture(
            "device_pair_start_item_ended.json",
            &DevicePairStartItem::Ended { pairing_id: pairing_id(), reason: PairEnd::Expired },
        ),
        fixture(
            "device_pair_confirm_result.json",
            &DevicePairConfirmResult { device_id: Some(device_id()) },
        ),
        fixture(
            "device_enroll_result.json",
            &DeviceEnrollResult { device_id: device_id(), scopes: phone_scopes() },
        ),
        fixture("device_prove_result.json", &DeviceProveResult { scopes: phone_scopes() }),
        fixture(
            "device_list_result.json",
            &DeviceListResult {
                devices: vec![
                    device_info(),
                    DeviceInfo { last_seen_at: None, revoked_at: None, ..device_info() },
                ],
            },
        ),
        fixture(
            "device_revoke_result.json",
            &DeviceRevokeResult { revoked_at: at("2026-10-26T10:00:00Z") },
        ),
    ]
}

/// The one-time token of the pairing samples: 16 bytes as unpadded base64url.
const PAIRING_TOKEN: &str = "q83vASNFZ4mrze8BI0VniQ";

/// The key fingerprint of the device samples.
const KEY_FINGERPRINT: &str = "SHA256:9f86d081884c7d65";

fn pairing_id() -> PairingId {
    parse("019a9b1c-3d00-7a10-8b20-00000000000b")
}

/// A P-256 public key in its 65-byte uncompressed form: `04`, then X and Y. The
/// coordinates are a pattern, not a point on the curve; the protocol does not check
/// them.
fn public_key() -> Vec<u8> {
    let mut key = vec![0x04];
    key.extend(1..=64u8);
    key
}

/// A DER signature, `SEQUENCE { INTEGER, INTEGER }`, with short integers.
fn signature() -> Vec<u8> {
    vec![0x30, 0x06, 0x02, 0x01, 0x2a, 0x02, 0x01, 0x2b]
}

/// The scopes of a phone.
fn phone_scopes() -> Vec<ScopeName> {
    vec![ScopeName::Read, ScopeName::Operate, ScopeName::Approve]
}

fn pairing_qr() -> PairingQr {
    PairingQr {
        daemon_id: daemon_id(),
        host: parse("100.101.102.103"),
        port: 7878,
        fqdn: Some("desk.tail1234.ts.net".into()),
        token: SecretText::new(PAIRING_TOKEN),
    }
}

fn device_info() -> DeviceInfo {
    DeviceInfo {
        device_id: device_id(),
        name: "Anna's iPhone".into(),
        key_fingerprint: KEY_FINGERPRINT.into(),
        node: "nPHONE1CNTRL".into(),
        scopes: phone_scopes(),
        created_at: at("2026-10-26T09:01:00Z"),
        last_seen_at: Some(at("2026-10-26T09:30:00Z")),
        revoked_at: Some(at("2026-10-26T10:00:00Z")),
    }
}

/// The prove facts of a remote `hello`.
pub(crate) fn prove_context() -> ProveContext {
    ProveContext {
        peer_node: "nPHONE1CNTRL".into(),
        daemon_node: "nDESK1CNTRL".into(),
        daemon_tailnet: "T1234CNTRL".into(),
    }
}

/// The `hello` result of a connection of the tailnet listener: the prove facts and
/// every key that a daemon announces to a phone.
pub(crate) fn remote_hello_result() -> HelloResult {
    HelloResult {
        daemon_id: daemon_id(),
        protocol: PROTOCOL_VERSION,
        version: "0.0.6".into(),
        capabilities: Capabilities {
            admin: None,
            screen_snapshots: None,
            search: Some(true),
            drafts_coalesced: Some(true),
            snapshot_live: Some(true),
            approval_local_only: Some(true),
            device_prove: Some(true),
            conversations_watch: None,
            extra: Default::default(),
        },
        paths: DaemonPaths {
            scratch_root: "/home/me/.local/share/efr/scratch".into(),
            data_dir: "/home/me/.local/share/efr".into(),
        },
        challenge: "3q2-7wAAAAAAAAAAAAAAAA".into(),
        prove: Some(prove_context()),
    }
}

/// The `hello` result of a connection of the tailnet listener of an efrd that answers
/// `conversations.watch`: [`remote_hello_result`] with its key.
pub(crate) fn watch_hello_result() -> HelloResult {
    let mut result = remote_hello_result();
    result.capabilities.conversations_watch = Some(true);
    result
}

/// A running turn in the middle of its second message, with characters of more than
/// one UTF-8 byte, so `text_bytes` differs from the count of characters and from the
/// count of UTF-16 units.
fn live_turn() -> LiveTurn {
    let text = "The largest file is ~/日志/größe.log 🙂".to_owned();
    LiveTurn {
        turn_id: turn_id(),
        after_seq: Seq::new(42),
        index: 1,
        text_bytes: text.len() as u64,
        text,
        reasoning: Some("**Checking the logs**\n\nThe journal holds most of it.".into()),
        reasoning_title: Some("Checking the logs".into()),
    }
}

/// An approval that only the user's own machine may answer.
pub(crate) fn local_only_approval_sample() -> Event {
    Event::ApprovalRequested {
        turn_id: turn_id(),
        call_id: call_id(),
        summary: "run `systemctl restart nginx` as root".into(),
        diff_preview: None,
        interactive: true,
        exit: None,
        expires_in_secs: None,
        local_only: true,
        reason: None,
    }
}

/// An approval that says why the turn's mode asks.
pub(crate) fn reason_approval_sample() -> Event {
    Event::ApprovalRequested {
        turn_id: turn_id(),
        call_id: call_id(),
        summary: "shell: run \"git push origin main\"".into(),
        diff_preview: None,
        interactive: false,
        exit: None,
        expires_in_secs: None,
        local_only: false,
        reason: Some(
            "cautious asks before a command that is not on its list of read-only commands and \
             any network access."
                .into(),
        ),
    }
}

/// The byte-exact vector of the message that a device signs for `device.prove`, for
/// clients in other languages: the inputs, and the message in lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProveVector {
    pub(crate) challenge: String,
    pub(crate) daemon_id: DaemonId,
    pub(crate) protocol: u32,
    pub(crate) device_id: DeviceId,
    pub(crate) peer_node: String,
    pub(crate) daemon_tailnet: String,
    pub(crate) message_hex: String,
}

impl ProveVector {
    /// The message of the vector's inputs.
    pub(crate) fn message(&self) -> ProveMessage<'_> {
        ProveMessage {
            challenge: &self.challenge,
            daemon_id: self.daemon_id,
            protocol: self.protocol,
            device_id: self.device_id,
            peer_node: &self.peer_node,
            daemon_tailnet: &self.daemon_tailnet,
        }
    }
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn prove_vector() -> ProveVector {
    let context = prove_context();
    let mut vector = ProveVector {
        challenge: "3q2-7wAAAAAAAAAAAAAAAA".into(),
        daemon_id: daemon_id(),
        protocol: PROTOCOL_VERSION,
        device_id: device_id(),
        peer_node: context.peer_node,
        daemon_tailnet: context.daemon_tailnet,
        message_hex: String::new(),
    };
    vector.message_hex = hex(&vector.message().to_bytes());
    vector
}

/// The vector of the QR code's text: the payload and its URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QrVector {
    pub(crate) qr: PairingQr,
    pub(crate) url: String,
}

pub(crate) fn qr_vector() -> QrVector {
    let qr = pairing_qr();
    let url = qr.url().expose_secret().to_owned();
    QrVector { qr, url }
}

/// The label of the project `app`.
fn app_label() -> RouteLabel {
    RouteLabel {
        scope: RouteScope::Project(project_id()),
        name: Some("app".into()),
        dir: "/home/me/p/app".into(),
    }
}

/// A list of recent conversations with every member set.
fn recent_list() -> ConversationsRecentResult {
    ConversationsRecentResult {
        scope: Some(app_label()),
        current: Some(conversation_id()),
        conversations: vec![recent_row()],
    }
}

/// A row of the list of recent conversations with every member set.
fn recent_row() -> RecentConversation {
    RecentConversation {
        summary: summary(),
        route: RouteScope::Project(project_id()),
        project: Some("app".into()),
        turns: 6,
        open_in: Some("/dev/pts/4".into()),
        here: true,
    }
}

/// A row of `conversations.watch`: a phone has no terminal, so no row is `here`.
fn watch_row() -> RecentConversation {
    RecentConversation { here: false, ..recent_row() }
}

/// A row of `conversations.watch` whose turn waits for an answer.
fn waiting_watch_row() -> RecentConversation {
    let mut row = watch_row();
    row.summary.status = ConversationStatus::AwaitingApproval;
    row.summary.last_seq = Seq::new(44);
    row.summary.updated_at = at("2026-10-03T12:00:09Z");
    row
}

/// The ranges of `disk` and `full` in the title of [`summary`].
fn title_ranges() -> Vec<TextRange> {
    vec![TextRange { start: 11, end: 15 }, TextRange { start: 16, end: 20 }]
}

/// A hit in an answer, with every member set.
fn answer_hit() -> SearchHit {
    SearchHit {
        kind: SearchKind::Answer,
        seq: Seq::new(40),
        turn_id: Some(turn_id()),
        at: at("2026-10-03T12:00:04Z"),
        history_cursor: PageCursor::new("s:41"),
        lines: vec![SearchLine {
            text: "The disk is full of old journal files: 3.9 GiB in /var/log/journal.".into(),
            ranges: vec![TextRange { start: 4, end: 8 }, TextRange { start: 12, end: 16 }],
        }],
    }
}

/// A search result with every member set.
fn search_list() -> ConversationsSearchResult {
    ConversationsSearchResult {
        scope: Some(app_label()),
        current: Some(conversation_id()),
        rows: vec![SearchRow {
            conversation: recent_row(),
            short_id: "019a9b1c".into(),
            title_ranges: title_ranges(),
            hit: Some(answer_hit()),
        }],
        partial: Some(SearchPartial::TimeCap),
        index: Some(SearchIndexState { percent: 40 }),
        stats: Some(SearchStats {
            rows_scanned: 1_240,
            conversations_scored: 96,
            titles_scored: 310,
            elapsed_us: 18_400,
        }),
    }
}

/// The hits of one conversation, newest first, with every member set: an answer with a
/// line that holds no match, then a command.
fn search_in_one() -> ConversationSearchResult {
    let mut answer = answer_hit();
    answer
        .lines
        .push(SearchLine { text: "Run journalctl --vacuum-size=500M.".into(), ranges: vec![] });
    ConversationSearchResult {
        title: Some(SearchLine { text: "why is the disk full".into(), ranges: title_ranges() }),
        hits: vec![
            answer,
            SearchHit {
                kind: SearchKind::Command,
                seq: Seq::new(38),
                turn_id: Some(turn_id()),
                at: at("2026-10-03T12:00:02Z"),
                history_cursor: PageCursor::new("s:39"),
                lines: vec![SearchLine {
                    text: "notify-send \"disk full\"".into(),
                    ranges: vec![
                        TextRange { start: 13, end: 17 },
                        TextRange { start: 18, end: 22 },
                    ],
                }],
            },
        ],
        partial: Some(SearchPartial::RowCap),
    }
}

/// A context report with every member set.
fn context_report() -> ConversationContextResult {
    let part = |kind, tokens, count| ContextPart { kind, tokens, count };
    ConversationContextResult {
        model: "claude-opus-5-5".into(),
        window: 1_000_000,
        window_known: true,
        trigger: 760_000,
        hard_cap: 950_000,
        auto: true,
        counted: Some(CountedUse {
            tokens: 98_400,
            turn_id: turn_id(),
            at: at("2026-10-20T18:05:00Z"),
        }),
        estimate: 96_700,
        running: true,
        parts: vec![
            part(ContextPartKind::System, 6_100, 1),
            part(ContextPartKind::Tools, 4_800, 9),
            part(ContextPartKind::Summary, 3_200, 2),
            part(ContextPartKind::Prompts, 2_000, 14),
            part(ContextPartKind::Preambles, 9_800, 14),
            part(ContextPartKind::Notices, 300, 1),
            part(ContextPartKind::Answers, 18_000, 14),
            part(ContextPartKind::ToolCalls, 4_100, 31),
            part(ContextPartKind::ToolOutputs, 48_300, 28),
            part(ContextPartKind::Pruned, 100, 3),
        ],
        largest: vec![ContextItem {
            kind: ContextPartKind::ToolOutputs,
            tokens: 12_300,
            turn: Some(12),
            tool: Some("shell".into()),
            label: Some("cargo test".into()),
        }],
        compactions: CompactionSummary {
            count: 1,
            newest: Some(CompactionBrief {
                at: at("2026-10-20T18:02:00Z"),
                trigger: CompactionTrigger::Auto,
                tokens_before: 231_000,
                tokens_after: 24_000,
                summarized: true,
                pruned_outputs: 12,
            }),
        },
        cache: CacheSummary {
            last_turn: Some(Usage {
                input_tokens: 98_000,
                output_tokens: 1_100,
                cached_input_tokens: 89_200,
                reasoning_tokens: 300,
                cache_write_tokens: 8_700,
                cache_write_1h_tokens: 8_700,
                context_tokens: 98_400,
            }),
            conversation: Usage {
                input_tokens: 1_310_000,
                output_tokens: 21_000,
                cached_input_tokens: 1_100_000,
                reasoning_tokens: 4_000,
                cache_write_tokens: 210_000,
                cache_write_1h_tokens: 200_000,
                context_tokens: 0,
            },
            turns: 14,
        },
    }
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
                max_context_window: Some(872_000),
                prefer_websockets: true,
            },
            ModelInfo {
                id: "gpt-5.5-preview".into(),
                efforts: Vec::new(),
                default_effort: None,
                default: false,
                source: ModelSource::Config,
                context_window: None,
                max_context_window: None,
                prefer_websockets: false,
            },
        ],
        catalog: Some(CatalogStatus {
            provider: Some("openai-subscription".into()),
            origin: CatalogOrigin::Cache,
            fetched_at: Some(at("2026-10-03T07:00:00Z")),
        }),
    }
}

/// One sample per event kind, and one of a kind from the future.
pub(crate) fn event_samples() -> Vec<Event> {
    let mut future = Map::new();
    future.insert("device_id".into(), json!(device_id()));
    future.insert("label".into(), json!("phone"));
    vec![
        Event::ConversationCreated {
            origin: Origin::Shell,
            tty: Some("/dev/pts/3".into()),
            route: Some(RouteScope::Project(project_id())),
        },
        Event::TerminalAttached {
            tty: "/dev/pts/7".into(),
            route: RouteScope::Project(project_id()),
            shell_pid: Some(4242),
            how: AttachHow::PickedUp,
        },
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
                blocked: vec![
                    Blocked {
                        host: "example.com".into(),
                        port: 443,
                        reason: BlockReason::NotAllowed,
                        count: 1,
                    },
                    Blocked {
                        host: "files.example.org".into(),
                        port: 443,
                        reason: BlockReason::UploadLimit,
                        count: 3,
                    },
                ],
                blocked_more: 2,
                proxied: true,
                surface_changes: vec![surface_change()],
                setup_error: Some("bwrap: Can't mount proc on /newroot/proc".into()),
                cache_deletes: vec!["/home/me/.cache/go-build/3f".into()],
                cache_deletes_more: 2,
                bus_refused: bus_refused(),
                bus_refused_more: 3,
            }),
            refusal: Some("efr's config (floor)".into()),
            changes: Some(file_changes()),
            diff: Some(
                "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    run();\n+    run_all();\n... 12 more lines\n"
                    .into(),
            ),
            left_running: true,
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
                always: Some("write under ~/Documents".into()),
            }),
            expires_in_secs: Some(120),
            local_only: false,
            reason: None,
        },
        Event::ApprovalResolved {
            turn_id: turn_id(),
            call_id: call_id(),
            decision: ApprovalDecision::Deny,
            origin: Origin::Phone,
        },
        Event::ApprovalExpired { turn_id: turn_id(), call_id: call_id(), away: true },
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
            changes: Some(file_changes()),
            undo: Some(turn_undo_info()),
        },
        Event::TurnCompleted {
            turn_id: turn_id(),
            usage: Some(Usage {
                input_tokens: 1200,
                output_tokens: 340,
                cached_input_tokens: 1024,
                reasoning_tokens: 128,
                cache_write_tokens: 160,
                cache_write_1h_tokens: 160,
                context_tokens: 89_000,
            }),
            context: Some(context_use()),
            changes: Some(file_changes()),
            stop: Some(TurnStop { reason: StopReason::Refusal, empty: true }),
            undo: Some(turn_undo_info()),
        },
        Event::TurnFailed {
            turn_id: turn_id(),
            error: ErrorBody::new(ErrorCode::Internal, "the provider stream ended early")
                .with_data(json!({ "provider": "openai", "status": 502 })),
            usage: Some(Usage::new(600, 0)),
            context: Some(context_use()),
            changes: Some(file_changes()),
            undo: Some(turn_undo_info()),
        },
        Event::TurnCancelled { turn_id: turn_id() },
        Event::ShellStarted { pty_id: pty_id(), cwd: "/home/me".into(), pid: Some(6060) },
        Event::ShellExited { pty_id: pty_id(), exit_code: Some(0) },
        Event::CwdChanged { pty_id: pty_id(), cwd: "/var/log".into(), host: Some("desk".into()) },
        Event::LoginCompleted { provider: "openai".into() },
        Event::LogoutCompleted { provider: "anthropic-api".into() },
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
            chain_step: Some(ChainStep::TurnModel),
            skipped: vec!["gpt-6-luna: missing".into()],
            reasked: true,
        },
        Event::AlwaysRuleSaved {
            turn_id: turn_id(),
            call_id: call_id(),
            outcome: AlwaysSaved::Saved { index: 2, rule: "write under ~/Documents".into() },
        },
        Event::ExitReviewStarted {
            turn_id: turn_id(),
            call_id: call_id(),
            kinds: vec![ExitKind::Write, ExitKind::Host],
        },
        Event::ClassifierPaused { turn_id: turn_id(), reason: PauseReason::InARow, denials: 3 },
        Event::PresenceChanged {
            conversation_id: conversation_id(),
            presence: Presence::Away,
            source: PresenceSource::Timeout,
        },
        Event::CacheDeleteApplied {
            turn_id: turn_id(),
            call_id: call_id(),
            paths: vec!["/home/me/.cache/go-build/3f".into(), "/home/me/.cache/go-build/a1".into()],
            freed_bytes: 1_288_490_188,
        },
        Event::HomeFileChanged {
            turn_id: turn_id(),
            call_id: call_id(),
            path: "/home/me/.zshrc".into(),
            floor: true,
            covered: true,
            backup: Some(
                "/home/me/.local/share/efr/file-backups/0190a8e2-7b1c-7d3e-8f00-1a2b3c4d5e6f/1/1/.zshrc"
                    .into(),
            ),
            real: Some("/home/me/dotfiles/zsh/.zshrc".into()),
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
        Event::SnapshotTaken {
            turn_id: turn_id(),
            root: "eggfriedrice.code".into(),
            phase: SnapshotPhase::Pre,
            commit: "9fceb02d0ae598e95dc970b74767f19372d61af8".into(),
            files: 1840,
            skipped: 3,
            ms: 41,
        },
        Event::UndoApplied {
            conversation_id: conversation_id(),
            undone_turn_id: turn_id(),
            undo_id: undo_id(),
            redo: true,
            origin: Origin::Cli,
            restored: 2,
            trashed: 1,
            deleted: 1,
            conflicts: 1,
            not_covered: 1,
            refs: vec![ref_change()],
            trash: Some(
                "/home/me/.local/share/efr/undo-trash/u-019a9b1c-3d00-7a10-8b20-00000000000f"
                    .into(),
            ),
            stopped: true,
        },
        Event::JobStarted {
            turn_id: turn_id(),
            call_id: call_id(),
            job: JobInfo {
                state: JobState::Running,
                ended_at: None,
                exit_code: None,
                stopped_by: None,
                output_bytes: 0,
                wakes: 0,
                forwards: Vec::new(),
                ..job_info()
            },
            launcher_pid: 48_211,
            launcher_start: 9_120_455,
        },
        Event::JobWatchMatched {
            job_id: JobId::nth(3),
            line: "error: listen EADDRINUSE: address already in use :::3000".into(),
            wakes_left: 2,
        },
        Event::JobPortForwarded {
            job_id: JobId::nth(3),
            listen: "127.0.0.1".into(),
            port: 8765,
            outcome: ForwardOutcome::InUse,
            by_job: Some(JobId::nth(2)),
            reason: Some("a service port".into()),
        },
        Event::JobEnded {
            job_id: JobId::nth(3),
            state: JobState::Stopped,
            exit_code: Some(143),
            stopped_by: Some(JobStopBy::Guard),
            output_bytes: 48_213,
            ran_secs: 1_920,
            tail: "ready in 412 ms\nGET / 200\n".into(),
            surface_changes: vec![surface_change()],
            quarantined: true,
            survivors: vec!["node".into()],
            launcher_lost: true,
            output_truncated: true,
            blocked: vec![Blocked {
                host: "<invalid>".into(),
                port: 443,
                reason: BlockReason::BadRequest,
                count: 4,
            }],
            blocked_more: 1,
            bus_refused: bus_refused(),
            bus_refused_more: 1,
        },
        Event::JobNotice {
            conversation_id: conversation_id(),
            turn_id: queued_turn_id(),
            job_id: JobId::nth(3),
            notice_kind: NoticeKind::Exited,
            woke: true,
            source_seq: Seq::new(812),
            not_user_input: true,
            text: "<job id=\"j3\" state=\"stopped\" command=\"npm run dev\">\nefr: this is a job \
                   notice, not a message from the user; no human input has occurred.\nGET / \
                   200\noutput: $SCRATCH/jobs/j3.out\n</job>"
                .into(),
        },
        Event::JobNoticeHeld {
            job_id: JobId::nth(3),
            source_seq: Seq::new(812),
            reason: HeldReason::WakeLimit,
        },
        Event::ShellCommandDetached {
            call_id: call_id(),
            command: "rsync -a -e ssh me@build:.cache/out ./out".into(),
            since: at("2026-10-03T10:00:01Z"),
            contained: false,
            input: InputWait::Hidden,
            looks_secret: true,
            prompt: "me@build's password: ".into(),
            approved_by: Some(ApprovedBy::You),
        },
        Event::ShellCommandInputChanged {
            call_id: call_id(),
            input: InputWait::Visible,
            contained: true,
            looks_secret: true,
            prompt: "Continue? [Y/n] ".into(),
        },
        Event::ShellCommandEnded {
            call_id: call_id(),
            exit_code: Some(130),
            how: ShellCommandEnd::Stopped,
        },
        Event::ConversationDeleted { origin: Origin::Cli },
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
