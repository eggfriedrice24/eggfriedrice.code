//! Events: the facts the daemon records in its event log and sends to subscribers.

use std::path::PathBuf;
use std::str::FromStr;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::compaction::is_zero_u64;
use crate::{
    CallId, ChainStep, CommandId, Compaction, ContextUse, ConversationId, EffectiveSettings,
    ErrorBody, ExitInfo, ExitKind, ExitRecord, ExitSource, FileChanges, Grant, JudgeKind, Launch,
    Origin, PauseReason, Presence, PresenceSource, PtyId, QuestionId, RefChange, ReportedFile,
    Risk, SandboxSummary, Scope, Seq, ShellContext, SnapshotPhase, SurfaceChange, TurnId,
    TurnSettings, TurnUndoInfo, UndoId, UserAuthorization, Verdict,
};

/// Something that happened, as the event log records it and subscribers receive it.
///
/// Variants are named `<Noun><PastParticiple>`. Each one carries the full state of what it
/// reports, never a delta against an earlier event, so a subscriber can resume from any
/// sequence number without reading anything else.
///
/// On the wire an event is one object whose `kind` member is the snake_case variant
/// name, such as `{"kind": "turn_started", ...}`. A kind that this build does not know
/// decodes as [`Event::Unknown`] and encodes back to the same JSON, so an old reader
/// keeps advancing its cursor past events from a newer daemon. A known kind with a
/// malformed body is an error, not `Unknown`.
///
/// A new variant needs a fixture in `fixtures/v1/events/`; a test fails until it has one.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::EnumDiscriminants,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
// NOTE: `EventKind` names every known kind without a hand-kept list. `Unknown` uses it
// to refuse a known kind, which keeps a malformed known event an error.
#[strum_discriminants(
    name(EventKind),
    vis(pub(crate)),
    derive(strum::EnumString, strum::IntoStaticStr),
    strum(serialize_all = "snake_case")
)]
#[non_exhaustive]
pub enum Event {
    /// A conversation began.
    ConversationCreated {
        /// The surface that started it.
        origin: Origin,
        /// The terminal it is the active conversation of, when it came from a shell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tty: Option<String>,
    },

    /// A prompt was accepted and waits for its turn. Prompts sent while a turn runs queue
    /// behind it.
    PromptQueued {
        /// The turn that will answer the prompt.
        turn_id: TurnId,
        /// The client's id for the `prompt.send` that carried it.
        command_id: CommandId,
        /// The prompt text.
        text: String,
        /// The surface that sent it.
        origin: Origin,
        /// The user's shell when the prompt was sent. A [`ShellContext`] holds no command
        /// line, so this event, which the log keeps forever and every subscriber receives,
        /// never carries the last command of `prompt.send`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ShellContext>,
        /// The settings that the prompt asked for, kept with it while it waits, so a held
        /// prompt runs with them too. Absent means none.
        #[serde(default, skip_serializing_if = "TurnSettings::is_empty")]
        settings: TurnSettings,
        /// The `turn_steered` events whose text this prompt carries, when an interrupt
        /// sent unread steers again as a prompt (`turn.interrupt` with
        /// `resend_steers`). A client shows those steers as this prompt from now on.
        /// Absent means none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        steers: Vec<Seq>,
    },

    /// A waiting prompt was taken out of the queue (`prompt.withdraw`, or
    /// `turn.interrupt` with `withdraw`). Its turn never starts, and this is the turn's
    /// last event.
    PromptWithdrawn {
        /// The turn of the withdrawn prompt.
        turn_id: TurnId,
        /// The surface that withdrew it.
        origin: Origin,
    },

    /// A queued prompt was held after a daemon restart. Only logs from earlier daemons
    /// hold it: a daemon now records a waiting prompt as not run with
    /// [`Event::TurnCancelled`] at the next start, and the one after that settles a held
    /// prompt the same way.
    PromptHeld {
        /// The turn of the held prompt.
        turn_id: TurnId,
    },

    /// A turn began running.
    TurnStarted {
        /// The turn.
        turn_id: TurnId,
        /// The user's working directory when the prompt was sent.
        cwd: PathBuf,
        /// The scope derived from that directory.
        scope: Scope,
        /// The settings that the turn runs with until it ends. Absent in turns recorded
        /// before turn settings, whose model and effort no event names.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<EffectiveSettings>,
    },

    /// The scope of the conversation changed between turns, because the user moved.
    ScopeChanged {
        /// The turn whose working directory produced the new scope.
        turn_id: TurnId,
        /// The scope before.
        from: Scope,
        /// The scope now.
        to: Scope,
    },

    /// An assistant message grew while the model streamed it. The event carries only
    /// the text added since the previous update, so a long answer costs the log its
    /// length once rather than once per update; `assistant_message_completed` carries
    /// the whole text. A reader that joined in the middle of a message (`offset` past
    /// what it holds) waits for the completed text.
    AssistantMessageUpdated {
        /// The turn.
        turn_id: TurnId,
        /// The position of the message among the turn's assistant messages, from 0.
        index: u32,
        /// The length in bytes of the message's text before `delta`.
        offset: u64,
        /// The text added at `offset`.
        delta: String,
    },

    /// An assistant message is complete.
    AssistantMessageCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The position of the message among the turn's assistant messages, from 0.
        index: u32,
        /// The whole text.
        text: String,
    },

    /// The model asked for a tool call.
    ToolCallStarted {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The tool's registered name, such as `shell` or `read_file`.
        tool: String,
        /// The tool's input as the model wrote it. For a call with `freeform`, a JSON
        /// string that holds the text.
        input: Value,
        /// True when the model wrote the input as plain text for a freeform tool, such
        /// as the patch of an `apply_patch` call for an OpenAI model: `input` is then a
        /// JSON string with that text, and the next request sends the call back in its
        /// freeform form. False when absent: `input` is the JSON that the model wrote,
        /// or a JSON string with arguments that were not JSON.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        freeform: bool,
        /// True when the call takes an input that the user chooses to type while it
        /// reports no wait (`input.respond` with `manual`), as a `shell` call does whose
        /// command does not type into a shell that reads command lines. A client offers
        /// such an input only for a call with this flag. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        manual_input: bool,
        /// How a `shell` call runs: typed into the hidden shell, in the `auto` sandbox
        /// with its grants, or in the exit child. Absent for other tools and in calls
        /// recorded before the sandbox.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch: Option<Launch>,
    },

    /// A running tool call produced more output. The daemon coalesces updates per call.
    ToolCallOutputUpdated {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The end of the output so far, bounded in size.
        tail: String,
        /// The size of the whole output so far, in bytes.
        bytes: u64,
    },

    /// A running tool call began or stopped waiting for the user to type a line. The
    /// daemon reports each change once. A client that can read keys asks the user while
    /// the call's latest `input` is not `none`, and sends the answer with
    /// `input.respond`; the call's `tool_call_completed` ends the wait as well.
    ToolCallInputChanged {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// What the call waits for now.
        input: InputWait,
        /// True for a `visible` wait whose prompt line looks like a password prompt
        /// (`password`, `passphrase`, `passcode`, `PIN`, `verification code` or
        /// `one-time code`) while the terminal is not in line mode, as behind a relay such
        /// as `sudo`'s own pty, `ssh` or `docker exec`. The program on the inner terminal
        /// decides whether the answer is shown, so a client hides what the user types
        /// and says so; the answer is still sent as a visible one. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        looks_secret: bool,
    },

    /// A tool call finished.
    ToolCallCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The output that the model sees, truncated in the middle when it is long.
        output: String,
        /// True when `output` was truncated; the full output stays in the recording.
        truncated: bool,
        /// True when the tool failed or the call was denied.
        is_error: bool,
        /// The exit code, for a tool that runs a command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// What a call through the sandbox's launcher did to the state around it: names
        /// only, never values. Absent for a call that did not use the launcher.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sandbox: Option<SandboxSummary>,
        /// Why efr refused the call before it ran, in a few words for a person, such as
        /// `efr's config (floor)`. Absent for a call that ran, and for one that the user
        /// denied or that no answer reached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refusal: Option<String>,
        /// The files that the call changed: for a file tool, the files it wrote, deleted
        /// or moved; for a `shell` call, what changed in the turn's registered project, `$SCRATCH` and,
        /// in `auto`, the registered projects that the line names, as efr's own
        /// snapshots before and after the call show it. Absent when nothing changed
        /// there, for a tool that cannot write, and when efr took no snapshot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<FileChanges>,
        /// The unified diff of a file tool's call: the diff of each file it changed, in
        /// the order it changed them, one after the other, each with its two header
        /// lines (`/dev/null` for a new or a deleted file), together at most
        /// [`MAX_CALL_DIFF_LINES`](crate::MAX_CALL_DIFF_LINES) lines and then a line
        /// `... N more lines`. Absent for other tools, for a binary file and when no
        /// content changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
    },

    /// A tool call needs the user's approval before it runs.
    ApprovalRequested {
        /// The turn, which waits for the answer.
        turn_id: TurnId,
        /// The call that waits.
        call_id: CallId,
        /// What the call would do, in one line.
        summary: String,
        /// A diff of the change, for a call that writes a file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_preview: Option<String>,
        /// True when the call may wait for input at the terminal, such as a `sudo`
        /// password: a client whose user approves it may keep the keys typed while it
        /// runs for the answer it asks for. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        interactive: bool,
        /// What the call would do outside the `auto` sandbox, for an exit. A client that
        /// does not know it shows `summary` as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<ExitInfo>,
    },

    /// The user answered an approval request.
    ApprovalResolved {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The answer.
        decision: ApprovalDecision,
        /// The surface that answered.
        origin: Origin,
    },

    /// An approval request can no longer be answered: it waited longer than its
    /// timeout, the turn was interrupted, or the daemon restarted while it was pending.
    /// The call did not run.
    ApprovalExpired {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// True when nobody answered while the user was away (`presence.away_wait_secs`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        away: bool,
    },

    /// The user added guidance to the running turn.
    TurnSteered {
        /// The turn.
        turn_id: TurnId,
        /// The guidance.
        text: String,
    },

    /// A model call of the turn read these steers: the daemon records it before or
    /// with the model call that sends them to the model. A client moves each steer
    /// from its unread list into the conversation. A steer that no
    /// `steering_delivered` names was never read by the model.
    SteeringDelivered {
        /// The turn.
        turn_id: TurnId,
        /// The `turn_steered` events that the call read, in sequence order.
        steers: Vec<Seq>,
    },

    /// The user took back steers of the turn that no model call read
    /// (`turn.interrupt` with `withdraw_steers`, Ctrl+C in the input row of a turn). No
    /// model call reads them, and they do not count as sent: a client shows them as
    /// taken back, as a `prompt_withdrawn` prompt.
    SteeringWithdrawn {
        /// The turn.
        turn_id: TurnId,
        /// The `turn_steered` events that the user took back, in sequence order.
        steers: Vec<Seq>,
        /// The surface that took them back.
        origin: Origin,
    },

    /// The user asked to interrupt the turn. The turn ends with
    /// [`Event::TurnInterrupted`] once the model's stream actually stops.
    TurnInterruptRequested {
        /// The turn.
        turn_id: TurnId,
        /// The surface that asked.
        origin: Origin,
    },

    /// An interrupted turn stopped.
    TurnInterrupted {
        /// The turn.
        turn_id: TurnId,
        /// The tokens that the turn used before it stopped, when the provider reported
        /// them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// How full the model's context was when the turn stopped. Absent in turns
        /// recorded before efr counted it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
        /// The files that the turn changed before it stopped, as for
        /// [`Event::TurnCompleted`]. Absent when nothing changed, and in turns recorded
        /// before efr reported it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<FileChanges>,
        /// What `efr undo` can take back of the turn. Absent when the turn has no
        /// snapshot, and in turns recorded before undo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<TurnUndoInfo>,
    },

    /// A turn finished normally.
    TurnCompleted {
        /// The turn.
        turn_id: TurnId,
        /// The tokens that the turn used, when the provider reported them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// How full the model's context was when the turn ended. Absent in turns
        /// recorded before efr counted it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
        /// The files that the turn changed: its first snapshot against its last, in the
        /// turn's registered project, `$SCRATCH` and the registered projects that its
        /// calls wrote. Absent when nothing changed there. `conversation.diff` returns
        /// the diff.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<FileChanges>,
        /// Why the model stopped, when its last call gave no text and no tool call, or
        /// stopped for another reason than the end of its answer. Absent for a normal
        /// answer, and in turns recorded before efr reported it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop: Option<TurnStop>,
        /// What `efr undo` can take back of the turn. Absent when the turn has no
        /// snapshot, and in turns recorded before undo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<TurnUndoInfo>,
    },

    /// A turn ended with an error, such as a provider failure.
    TurnFailed {
        /// The turn.
        turn_id: TurnId,
        /// What went wrong.
        error: ErrorBody,
        /// The tokens that the turn used before it failed, when the provider reported
        /// them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// How full the model's context was when the turn failed. Absent in turns
        /// recorded before efr counted it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<ContextUse>,
        /// The files that the turn changed before it failed, as for
        /// [`Event::TurnCompleted`]. Absent when nothing changed, and in turns recorded
        /// before efr reported it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changes: Option<FileChanges>,
        /// What `efr undo` can take back of the turn. Absent when the turn has no
        /// snapshot, and in turns recorded before undo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<TurnUndoInfo>,
    },

    /// A turn that was running or waiting when the daemon stopped was cancelled at the
    /// next start. Without a [`Event::TurnStarted`] before it, the turn never ran: its
    /// prompt was queued, and the user is told to send it again. Nothing continues on
    /// its own after a restart.
    TurnCancelled {
        /// The turn.
        turn_id: TurnId,
    },

    /// The conversation's hidden shell started.
    ShellStarted {
        /// The shell's PTY, which `pty.attach` takes.
        pty_id: PtyId,
        /// The directory the shell started in.
        cwd: PathBuf,
        /// The shell's process id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
    },

    /// The conversation's hidden shell exited.
    ShellExited {
        /// The shell's PTY.
        pty_id: PtyId,
        /// The exit code; absent when a signal ended the shell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    /// The hidden shell reported a new working directory (OSC 7).
    CwdChanged {
        /// The shell's PTY.
        pty_id: PtyId,
        /// The new directory.
        cwd: PathBuf,
        /// The host in the report, when it named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },

    /// A provider login finished and its credentials are stored.
    LoginCompleted {
        /// The provider, such as `openai`.
        provider: String,
    },

    /// `admin.logout` deleted the stored credentials of a provider. It holds no part of
    /// them.
    LogoutCompleted {
        /// The provider, such as `anthropic-api`.
        provider: String,
    },

    /// A `shell` call in `auto` leaves the sandbox: the exits, what an approval opens,
    /// and the record that a classifier would judge (phase A), stored once per call.
    ExitRequested {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The exits of the call.
        kinds: Vec<ExitKind>,
        /// Exactly what an approval opens.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        grants: Vec<Grant>,
        /// Where the exits came from.
        source: ExitSource,
        /// The facts of the exit. Boxed, because it is much larger than every other
        /// event.
        record: Box<ExitRecord>,
    },

    /// An exit was judged: by the user, by a floor before any question, by the
    /// classifier (phase A) or by an always-allow rule (phase E).
    ExitJudged {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// Who judged.
        judge: JudgeKind,
        /// The verdict.
        verdict: Verdict,
        /// The classifier's model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// How long the classifier took, in milliseconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency_ms: Option<u64>,
        /// The classifier's risk.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk: Option<Risk>,
        /// The classifier's view of the user's authorization.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_authorization: Option<UserAuthorization>,
        /// The classifier's policy category, or the floor's rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
        /// The classifier's reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
        /// The SHA-256 of the record the classifier saw, in hex.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record_sha256: Option<String>,
        /// True when the verdict came from the turn's cache.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cached: bool,
        /// The step of the classifier's chain that gave the verdict.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chain_step: Option<ChainStep>,
        /// The models that the classifier's chain skipped, each as `<model>: <reason>`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skipped: Vec<String>,
    },

    /// The classifier started to review an exit, so a client can show `reviewing:`. A
    /// verdict from the turn's cache has none.
    ExitReviewStarted {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The exits of the call.
        kinds: Vec<ExitKind>,
    },

    /// The classifier stopped for the rest of the turn after too many denials: each
    /// later exit asks the user, or stops the turn while the user is away.
    ClassifierPaused {
        /// The turn.
        turn_id: TurnId,
        /// Why.
        reason: PauseReason,
        /// The denials that the limit counted: in a row, or in the turn.
        denials: u8,
    },

    /// The presence of a conversation changed: a person is there to answer its questions,
    /// or not.
    PresenceChanged {
        /// The conversation.
        conversation_id: ConversationId,
        /// The new state.
        presence: Presence,
        /// What changed it.
        source: PresenceSource,
    },

    /// efrd deleted paths in the real tool caches that a call had deleted in its private
    /// layer, after the classifier or the user allowed it. Names only.
    CacheDeleteApplied {
        /// The turn.
        turn_id: TurnId,
        /// The call that deleted them in its layer.
        call_id: CallId,
        /// The deleted paths, at most [`SandboxSummary::MAX_CACHE_DELETES`].
        paths: Vec<PathBuf>,
        /// The bytes that the deletes freed.
        freed_bytes: u64,
    },

    /// A call changed a file outside the project, such as `~/.zshrc`, for the line at
    /// the end of the turn. Names only. Phase A sent it for a file that runs code later,
    /// with `backup`. From phase B efrd sends it for each changed file outside the
    /// roots, with `covered` and no `backup`.
    HomeFileChanged {
        /// The turn.
        turn_id: TurnId,
        /// The call.
        call_id: CallId,
        /// The file, absolute.
        path: PathBuf,
        /// True when the file is a floor.
        floor: bool,
        /// True when efr copied the file into its home store before the call, so
        /// `efr undo` can bring it back.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        covered: bool,
        /// The copy of the file before the call, in efr's phase A backup store. Absent
        /// from phase B.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backup: Option<PathBuf>,
        /// The real file, absolute, when `path` reaches it through a link, such as a
        /// dotfile link into `~/dotfiles`. Absent when `path` is the real file, and in
        /// events recorded before efr reported it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        real: Option<PathBuf>,
    },

    /// The surface guard found git settings or other files that run code, which a call
    /// changed. Names only.
    SandboxSurfaceChanged {
        /// The turn.
        turn_id: TurnId,
        /// The call that made the changes.
        call_id: CallId,
        /// The changes.
        changes: Vec<SurfaceChange>,
        /// True when at least one change went to quarantine.
        quarantined: bool,
    },

    /// The turn asks the user whether to keep quarantined changes before its next
    /// call. It is not an approval of a tool call: the call already ended.
    SurfaceQuestionRequested {
        /// The turn, which waits for the answer.
        turn_id: TurnId,
        /// The call that made the changes; it does not wait.
        call_id: CallId,
        /// The question, which `sandbox.surface_respond` answers.
        question_id: QuestionId,
        /// The quarantined changes.
        changes: Vec<SurfaceChange>,
    },

    /// A quarantine question was answered, expired or ended with an interrupt. Only a
    /// "keep" from the user moves the changes back.
    SurfaceQuestionAnswered {
        /// The turn.
        turn_id: TurnId,
        /// The question.
        question_id: QuestionId,
        /// True when the changes moved back; false leaves them in quarantine.
        keep: bool,
        /// The surface that answered; absent when the question expired or the turn was
        /// interrupted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<Origin>,
    },

    /// At the end of an `auto` turn: the files it changed that run code later outside
    /// the sandbox.
    TurnSurfaceReport {
        /// The turn.
        turn_id: TurnId,
        /// The files.
        files: Vec<ReportedFile>,
    },

    /// efrd compacted the conversation's context: it pruned old tool output, wrote a
    /// summary of the history before a cut, or both. The model's next request starts
    /// from the compaction (the README of `efr-conversation`, section "Context"). Every
    /// event stays in the log; `efr history` marks the place.
    ConversationCompacted(Compaction),

    /// The sandbox probe's result changed to unavailable: `auto` turns run as
    /// `cautious` until it is available again.
    SandboxUnavailable {
        /// Why, in one sentence.
        reason: String,
    },

    /// efr took a snapshot of a root for a turn: once per root before the turn's first
    /// call that can write it and once at the turn's end, both sent at the turn's end,
    /// and once per copy of files outside the roots.
    SnapshotTaken {
        /// The turn.
        turn_id: TurnId,
        /// The root as a client shows it: the project's name, `$SCRATCH`, or `home` for
        /// a copy of files outside the roots.
        root: String,
        /// Before or after.
        phase: SnapshotPhase,
        /// The snapshot's commit in efr's store, in hex.
        commit: String,
        /// The files in the snapshot.
        files: u32,
        /// The files that the snapshot left out, such as large or ignored files.
        skipped: u32,
        /// How long the snapshot took, in milliseconds.
        ms: u64,
    },

    /// `turn.undo` took back the changes of a turn, or a redo took back an undo. It
    /// belongs to the conversation, not to a turn: `undone_turn_id` names the turn.
    UndoApplied {
        /// The conversation.
        conversation_id: ConversationId,
        /// The turn whose changes the undo took back.
        undone_turn_id: TurnId,
        /// The undo.
        undo_id: UndoId,
        /// True for a redo. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redo: bool,
        /// The surface that asked.
        origin: Origin,
        /// The files that the undo wrote back.
        restored: u32,
        /// The files that the undo moved to the undo trash.
        trashed: u32,
        /// The links and empty entries that the undo removed.
        deleted: u32,
        /// The paths that the undo skipped, because they changed after the turn.
        conflicts: u32,
        /// The paths that no snapshot covers, so the undo left them as they are.
        not_covered: u32,
        /// The refs that the undo moved, or tried to. Absent when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refs: Vec<RefChange>,
        /// The directory of the undo trash. Absent when the undo put nothing there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trash: Option<PathBuf>,
        /// True when the undo stopped at a write that failed, after it changed some
        /// paths; the next `efr undo` finishes it. False when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stopped: bool,
    },

    /// An event of a kind that this build does not know. It encodes back to the same JSON
    /// object it was decoded from. The schema leaves it out: it describes the known
    /// kinds, and says that readers must accept others.
    #[serde(untagged)]
    #[schemars(skip)]
    Unknown {
        /// The `kind` member, never one of the kinds above.
        #[serde(deserialize_with = "unknown_kind")]
        kind: String,
        /// Every other member. A `kind` key in it is not written.
        #[serde(flatten, serialize_with = "payload_without_kind")]
        payload: Map<String, Value>,
    },
}

impl Event {
    /// The wire name of the event's kind, such as `turn_started`. The event log stores
    /// it in its `kind` column.
    pub fn kind(&self) -> &str {
        match self {
            Event::Unknown { kind, .. } => kind,
            known => {
                let name: &'static str = EventKind::from(known).into();
                name
            }
        }
    }

    /// The turn that the event belongs to, when it belongs to one.
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            Event::PromptQueued { turn_id, .. }
            | Event::PromptWithdrawn { turn_id, .. }
            | Event::PromptHeld { turn_id }
            | Event::TurnStarted { turn_id, .. }
            | Event::ScopeChanged { turn_id, .. }
            | Event::AssistantMessageUpdated { turn_id, .. }
            | Event::AssistantMessageCompleted { turn_id, .. }
            | Event::ToolCallStarted { turn_id, .. }
            | Event::ToolCallOutputUpdated { turn_id, .. }
            | Event::ToolCallInputChanged { turn_id, .. }
            | Event::ToolCallCompleted { turn_id, .. }
            | Event::ApprovalRequested { turn_id, .. }
            | Event::ApprovalResolved { turn_id, .. }
            | Event::ApprovalExpired { turn_id, .. }
            | Event::TurnSteered { turn_id, .. }
            | Event::SteeringDelivered { turn_id, .. }
            | Event::SteeringWithdrawn { turn_id, .. }
            | Event::TurnInterruptRequested { turn_id, .. }
            | Event::TurnInterrupted { turn_id, .. }
            | Event::TurnCompleted { turn_id, .. }
            | Event::TurnFailed { turn_id, .. }
            | Event::TurnCancelled { turn_id }
            | Event::ExitRequested { turn_id, .. }
            | Event::ExitJudged { turn_id, .. }
            | Event::ExitReviewStarted { turn_id, .. }
            | Event::ClassifierPaused { turn_id, .. }
            | Event::CacheDeleteApplied { turn_id, .. }
            | Event::HomeFileChanged { turn_id, .. }
            | Event::SandboxSurfaceChanged { turn_id, .. }
            | Event::SurfaceQuestionRequested { turn_id, .. }
            | Event::SurfaceQuestionAnswered { turn_id, .. }
            | Event::TurnSurfaceReport { turn_id, .. }
            | Event::SnapshotTaken { turn_id, .. } => Some(*turn_id),
            Event::ConversationCompacted(compaction) => compaction.turn_id,
            // NOTE: an undo runs between turns; a view that follows a turn must not end
            // or show it as part of the undone turn.
            Event::UndoApplied { .. }
            | Event::ConversationCreated { .. }
            | Event::ShellStarted { .. }
            | Event::ShellExited { .. }
            | Event::CwdChanged { .. }
            | Event::LoginCompleted { .. }
            | Event::LogoutCompleted { .. }
            | Event::SandboxUnavailable { .. }
            | Event::PresenceChanged { .. }
            | Event::Unknown { .. } => None,
        }
    }
}

/// Writes the payload of [`Event::Unknown`]. A payload built by hand may hold its own
/// `kind`, and writing it would give the object two `kind` members.
fn payload_without_kind<S: Serializer>(
    payload: &Map<String, Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(payload.iter().filter(|(key, _)| key.as_str() != "kind"))
}

/// Reads the `kind` of [`Event::Unknown`], refusing a kind that this build knows.
///
/// serde tries the untagged `Unknown` variant after every tagged variant has failed,
/// which also happens when a known kind has a malformed body. Refusing known kinds here
/// turns that case into an error instead of a silent `Unknown`. The price is serde's
/// generic message for it, because serde drops the errors of the variants it tried.
fn unknown_kind<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let kind = String::deserialize(deserializer)?;
    match EventKind::from_str(&kind) {
        Ok(known) if known != EventKind::Unknown => Err(de::Error::custom(format!(
            "the body of the `{kind}` event does not match its kind"
        ))),
        _ => Ok(kind),
    }
}

/// An event with its place in the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventEnvelope {
    /// The event's global sequence number.
    pub seq: Seq,
    /// The conversation the event belongs to; absent for daemon-wide events such as
    /// [`Event::LoginCompleted`] and [`Event::LogoutCompleted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// When the daemon recorded the event.
    pub at: Timestamp,
    /// The event.
    pub event: Event,
}

/// The user's answer to an approval request.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApprovalDecision {
    /// Run the tool call.
    Allow,
    /// Do not run it; the model sees a denied call.
    Deny,
}

/// Whether a running tool call's command waits for the user to type a line, and what
/// happens to that line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputWait {
    /// The call does not wait for input now, so a client stops asking the user.
    #[default]
    None,
    /// The command waits for an answer that is not read like a password, such as a
    /// `[Y/n]` question. On a plain terminal the answer is echoed and reaches the output
    /// that the model reads. Through a relay such as `sudo`'s own pty, the program on
    /// the inner terminal decides whether the answer is shown, and the model then reads
    /// what is shown; a password prompt of a program behind such a relay is reported
    /// as visible too.
    Visible,
    /// The command reads a line with echo off, such as the password prompt of `sudo` or
    /// `ssh`. The user's answer is not shown and the terminal does not put it in the
    /// output; the model sees it only if the program that reads it prints it.
    Hidden,
}

/// Tokens that a turn used, as the provider reported them.
///
/// `input_tokens`, `output_tokens`, `cached_input_tokens`, `reasoning_tokens`,
/// `cache_write_tokens` and `cache_write_1h_tokens` are sums over the model calls of the
/// turn. `context_tokens` is not a sum: it is the size of the context at the last call.
/// The cache counts are parts of `input_tokens` for every provider, so
/// `cached_input_tokens / input_tokens` is the part of the input that the cache served.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Tokens sent to the model, cached ones included.
    pub input_tokens: u64,
    /// Tokens the model produced, reasoning included.
    pub output_tokens: u64,
    /// The part of `input_tokens` that the provider served from its prompt cache. Absent
    /// when none, or when the provider did not say.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cached_input_tokens: u64,
    /// The part of `output_tokens` that the model spent on reasoning. Absent when none,
    /// or when the provider did not say.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub reasoning_tokens: u64,
    /// The part of `input_tokens` that the provider wrote to its prompt cache, with
    /// every time to live. Absent when none, or when the provider did not say.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cache_write_tokens: u64,
    /// The part of `cache_write_tokens` written with a time to live of one hour. Absent
    /// when none, and always for a provider without such a choice, such as OpenAI.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub cache_write_1h_tokens: u64,
    /// The tokens in the context at the last model call: its input plus its output.
    /// Absent when no call reported its usage, and in turns recorded before efr counted
    /// it.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub context_tokens: u64,
}

impl Usage {
    /// The usage of `input_tokens` and `output_tokens`, with the other counts zero.
    pub const fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            cache_write_tokens: 0,
            cache_write_1h_tokens: 0,
            context_tokens: 0,
        }
    }
}

/// Why the model of a completed turn stopped, when a client shows it: the last model
/// call gave no answer, or it stopped for another reason than the end of its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TurnStop {
    /// The stop reason of the last model call.
    pub reason: StopReason,
    /// True when the last model call gave no text and no tool call.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
}

/// Why a model call stopped, as the provider reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The model finished its answer.
    EndTurn,
    /// The answer reached the output token limit.
    MaxTokens,
    /// The model refused to answer.
    Refusal,
    /// The provider stopped the answer for its content.
    ContentFilter,
    /// The provider paused a long turn, which efr does not continue.
    PauseTurn,
}

impl StopReason {
    /// The wire form, such as `max_tokens`.
    pub const fn as_str(self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::Refusal => "refusal",
            StopReason::ContentFilter => "content_filter",
            StopReason::PauseTurn => "pause_turn",
        }
    }
}

#[cfg(test)]
mod tests;
