//! One turn, from a prompt to its terminal event, and the single permission check
//! point.
//!
//! A turn reads a snapshot of the conversation, derives the scope from the user's
//! working directory, records `turn_started` (and `scope_changed` when the user moved),
//! makes sure `$SCRATCH` exists, and assembles the request: the system prompt, bounded
//! history, the live-state preamble with the prompt, and the tool definitions. Before
//! each model call it estimates the request and guards it: it compacts at the trigger,
//! never sends a request above the hard cap, and compacts once and sends the call once
//! more when the provider refuses it as too large (the README, section "Context"). It
//! then streams the provider and records what comes back. Each tool call the model asks for
//! is recorded, goes through [`Turn::authorize_tool_call`], runs when allowed or
//! approved, and its result goes back to the model, until the model answers without a
//! tool call. The turn ends with `turn_completed`, `turn_failed` or
//! `turn_interrupted`: it builds that batch and hands it to the actor, which marks the
//! turn as ended first and records the batch after, so a client that sees the end
//! finds no running turn.
//!
//! `authorize_tool_call` is the only place in efr where a tool call meets the
//! permission engine. Tools declare (`efr-tools`, through the daemon's [`Toolbox`]),
//! `efr-permissions` decides, and this file enforces; nothing reaches
//! [`Toolbox::invoke`] without passing it.
//!
//! In `auto` a shell call runs in the kernel sandbox. `Contain` runs it at once with
//! [`Launch::Contained`]. An exit goes to the classifier first when it may judge it
//! (`review.rs`), and else, or when it is unsure, to the user, with its record in
//! `exit_requested` first; an allow runs the call with the narrowest launch
//! ([`exit::grant`]). An exit that would run outside the sandbox must be one command, or
//! the call is refused with no question. A floor refuses an exit before any question,
//! and three refusals in a row without a person stop the turn; classifier denials pause
//! the classifier for the turn. A question waits less while the user is away. A call
//! that changed git settings that run programs asks the user whether to keep them
//! before the next call, and its deletes in a private cache layer may reach the real
//! caches (`cache.rs`).
//!
//! In full mode a call runs as the user with no sandbox. The engine lets through what
//! the sandbox would hold; the turn records the exits that it let through, checks the
//! gate before each call, and asks the user, at their own machine, before a delete that
//! full mode guards (`full.rs`).

mod always;
mod cache;
mod coalesce;
mod compact;
mod drafter;
mod full;
mod review;
mod stream;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_permissions::{
    AllowRule, AlwaysVerdict, Approver, Cause, ConversationPolicy, Decision, DecisionInput, Effect,
    Engine, Requirements, runs_as_another_user,
};
use efr_protocol::{
    ApprovalDecision, CallId, ConversationId, EffectiveSettings, ErrorBody, ErrorCode, Event,
    ExitKind, InputWait, JobInfo, JudgeKind, Launch, Mode, Origin, Presence, QuestionId, Scope,
    ShellContext, StopReason as WireStop, SurfaceChange, TurnId, TurnSettings, TurnStop, Verdict,
};
use efr_provider::{
    ContentBlock, EditTool, Message, Provider, ProviderError, Request, Role, StopReason, TokenUsage,
};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Stopwatch;
use efr_store::conversations::TurnStatus;
use efr_store::turn_messages::NewTurnMessages;
use efr_store::{Batch, Committed};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tracing::Instrument as _;

pub(crate) use self::compact::{read_fresh, summarized, wire_usage};

use self::coalesce::{Coalescer, sleep_or_pending};
use self::compact::{Guard, Recovered};
use self::drafter::Drafter;
use self::review::Reviewed;
use self::stream::Response;
use crate::approvals::{self, Approvals};
use crate::compaction::{Placed, Window, with_window};
use crate::context::{self, Meter};
use crate::drafts::LiveStatus;
use crate::exit::{self, TurnExits};
use crate::fresh::Fresh;
use crate::gap::CallGap;
use crate::history::{CachedTurn, ModelKey, Snapshot, close_open_calls, end_marker};
use crate::interrupt::Interrupt;
use crate::preamble::LiveState;
use crate::presence::PRESENCE_CHECK;
use crate::questions::Questions;
use crate::scratch::Scratch;
use crate::settings::{self, Place};
use crate::steer::Steering;
use crate::wake;
use crate::{
    CallContext, ConfigSource, ConversationConfig, ConversationDeps, ConversationError, FileBackup,
    JobNoticeInput, OutputSink, ToolCall, ToolOutcome, Toolbox, TurnChanges,
};

/// The longest output tail in a `tool_call_output_updated` event, in bytes.
const TAIL_MAX: usize = 4096;

/// How many changes of a call's input wait may wait for the turn to record them. A
/// command changes it at most once a second, and the turn records each at once.
const INPUT_CAPACITY: usize = 64;

/// The `provider_options` key of the prompt cache key. Only the OpenAI provider reads
/// it; another provider ignores it. The turn sends the conversation id there unless
/// the config names a key.
pub(crate) const PROMPT_CACHE_KEY: &str = "prompt_cache_key";

/// What the model reads for a call that did not run because the user interrupted the
/// turn.
const NOT_RUN: &str = "The user interrupted the turn before this call ran.";

/// What the model reads for a call that was running when the user interrupted the turn.
pub(crate) const STOPPED: &str =
    "The user interrupted the turn while this call ran; it was stopped.";

/// What the summary of a question in a wake turn starts with.
pub(crate) const WAKE_QUESTION: &str = "(job notice turn) ";

/// What the model reads for a call whose approval request expired unanswered.
const EXPIRED: &str = "The approval request expired before the user answered; the call did \
                       not run.";

/// What every turn of one conversation shares.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) conversation_id: ConversationId,
    /// Read once per turn, when it starts.
    pub(crate) config: Arc<dyn ConfigSource>,
    pub(crate) deps: ConversationDeps,
    pub(crate) scratch: Mutex<Scratch>,
    pub(crate) approvals: Approvals,
    pub(crate) questions: Questions,
    /// The compactions in a row that missed, carried from turn to turn so that the
    /// breaker stays open until the context is below the trigger again.
    pub(crate) misses: AtomicU32,
    /// The context and compacting drafts of the running turn, for a client that
    /// attaches while it runs.
    pub(crate) status: Arc<LiveStatus>,
    /// When the newest model call started, for the gap before the next one in the log.
    pub(crate) gap: CallGap,
    /// The paths whose delete in the real tool caches the classifier or the user refused
    /// in this conversation: a later call does not raise them again.
    pub(crate) denied_cache_deletes: Mutex<HashSet<PathBuf>>,
}

/// A prompt that waits for its turn, or runs as one; or a turn that job notices
/// started.
///
/// `Debug` leaves out the last command, which can hold a secret.
#[derive(Clone)]
pub(crate) struct TurnSpec {
    pub(crate) turn_id: TurnId,
    /// The prompt; empty for a turn that job notices started.
    pub(crate) text: String,
    pub(crate) origin: Origin,
    pub(crate) context: Option<ShellContext>,
    /// The last command of `prompt.send`. It lives only in memory, for the preamble.
    pub(crate) last_command: Option<String>,
    /// The settings the prompt asked for, resolved when the turn starts.
    pub(crate) settings: TurnSettings,
    /// What started the turn: a prompt, or job notices.
    pub(crate) kind: TurnKind,
    /// The job notices that the turn takes into its first message, after the prompt.
    /// It records their `job_notice` events before its `turn_started`.
    pub(crate) notices: Vec<JobNoticeInput>,
    /// How many more notices wait, which the first message names in one line.
    pub(crate) more_notices: usize,
    /// The classifier's denial counts of the turn that notices started before this
    /// one, which a turn that notices start goes on with (phase C).
    pub(crate) carried: Option<TurnExits>,
}

/// What started a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnKind {
    /// A prompt of the user.
    Prompt,
    /// Job notices, while the conversation was idle (a wake turn). Nobody asked for its
    /// steps now. It runs in `cwd`, the directory of the turn that started the job.
    Wake {
        /// The user's directory when the job's turn started.
        cwd: PathBuf,
    },
}

impl TurnSpec {
    /// True for a turn that job notices started.
    pub(crate) fn is_wake(&self) -> bool {
        matches!(self.kind, TurnKind::Wake { .. })
    }
}

impl fmt::Debug for TurnSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: last_command is deliberately missing; see the type's doc comment.
        f.debug_struct("TurnSpec")
            .field("turn_id", &self.turn_id)
            .field("text", &self.text)
            .field("origin", &self.origin)
            .field("context", &self.context)
            .field("settings", &self.settings)
            .field("kind", &self.kind)
            .field("notices", &self.notices.len())
            .finish_non_exhaustive()
    }
}

/// How the actor reaches a running turn. Cheap to clone; clones share it.
#[derive(Debug, Clone)]
pub(crate) struct Control {
    pub(crate) interrupt: Interrupt,
    pub(crate) steering: Steering,
}

impl Control {
    pub(crate) fn new() -> Self {
        Control { interrupt: Interrupt::new(), steering: Steering::default() }
    }
}

/// What a finished turn hands back to the actor.
#[derive(Debug)]
pub(crate) struct TurnEnd {
    pub(crate) turn_id: TurnId,
    /// The exact messages of the turn, for the history of later turns; `None` when
    /// the turn stopped before it was recorded as started.
    pub(crate) cached: Option<CachedTurn>,
    /// The terminal event and what goes with it. The actor records it after it marks
    /// the turn as ended, so no client sees the end while the turn still counts as
    /// running.
    pub(crate) record: Batch,
    /// The fresh context block of the newest compaction, which the actor keeps for the
    /// next turns; `None` when the turn neither read nor made one.
    pub(crate) fresh: Option<Fresh>,
    /// The earlier turns that the turn's newest request carried, after any compaction
    /// of the turn: the only ones that the next turn can need from the actor's cache.
    /// `None` when the turn stopped before it built a request.
    pub(crate) carried: Option<HashSet<TurnId>>,
    /// The background jobs that the turn's calls started.
    pub(crate) jobs: Vec<JobInfo>,
    /// The mode the turn ran with; `None` when it stopped before it resolved one.
    pub(crate) mode: Option<Mode>,
    /// The user's directory of the turn.
    pub(crate) cwd: PathBuf,
    /// The exits, refusals and classifier denials of the turn.
    pub(crate) exits: TurnExits,
    /// The job notices that the turn did not record because it stopped before it
    /// started; they wait for the next turn.
    pub(crate) untaken: Vec<JobNoticeInput>,
}

/// Runs the turn `spec` to its terminal event.
pub(crate) async fn run(
    shared: Arc<Shared>,
    spec: TurnSpec,
    control: Control,
    cache: HashMap<TurnId, Arc<CachedTurn>>,
    fresh: Option<Fresh>,
) -> TurnEnd {
    let span = tracing::info_span!(
        "turn",
        conversation_id = %shared.conversation_id,
        turn_id = %spec.turn_id,
    );
    async move {
        let mut turn = Turn::new(shared, spec, control, fresh);
        let ending = match turn.drive(&cache).await {
            Ok(ending) => ending,
            Err(error) => {
                tracing::error!(error = %error, "the turn could not go on");
                Ending::Failed(ErrorBody::new(ErrorCode::Internal, error.to_string()))
            }
        };
        turn.finish(ending).await
    }
    .instrument(span)
    .await
}

/// How a turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ending {
    Completed,
    Failed(ErrorBody),
    Interrupted,
}

impl Ending {
    /// The status of a turn that ends so.
    fn status(&self) -> TurnStatus {
        match self {
            Ending::Completed => TurnStatus::Completed,
            Ending::Failed(_) => TurnStatus::Failed,
            Ending::Interrupted => TurnStatus::Interrupted,
        }
    }
}

/// The answer of the check point about one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Authorization {
    /// The engine allowed or contained the call, or the user approved it;
    /// `approved_interactive` when the user approved a call that may wait for input at
    /// the terminal, and `launch` how it runs.
    Allowed { approved_interactive: bool, launch: Launch },
    /// The engine refused the call, or the user denied it; the model reads `message`,
    /// and the user `refusal` when the engine refused it.
    Denied { message: String, refusal: Option<String> },
    /// The call could not be judged, such as an unknown tool, or a rule refused it
    /// before any question; the model reads `message`, and the user `refusal` for a
    /// rule's refusal.
    Refused { message: String, refusal: Option<String> },
    /// The user interrupted the turn while the call waited for approval.
    Interrupted,
    /// The approval request expired unanswered; `away` when nobody answered while the
    /// user was away, so the model goes on without the call.
    Expired { away: bool },
}

/// One running turn.
struct Turn {
    shared: Arc<Shared>,
    /// The settings of the turn, read when it started; a later change waits for the
    /// next turn.
    config: Arc<ConversationConfig>,
    /// The mode, the model and the effort of the turn, resolved when it starts; `None`
    /// until then.
    settings: Option<EffectiveSettings>,
    /// The tool with which the turn's model changes files, from the provider's model
    /// list when the turn starts: the request offers only this one of the two edit
    /// tools, and each call carries it so the toolbox refuses the other one.
    edit_tool: EditTool,
    spec: TurnSpec,
    control: Control,
    cwd: PathBuf,
    scope: Scope,
    scratch: PathBuf,
    /// The turn's messages for the cache: the prompt as the model read it, with the
    /// preamble (only its text when the turn stopped before the preamble was made),
    /// then every message after it.
    transcript: Vec<Message>,
    /// The position of the next assistant message with text.
    assistant_index: u32,
    /// How many bytes of that message's text `assistant_message_updated` events hold.
    streamed: usize,
    /// The drafts of the turn, for live clients only.
    drafter: Drafter,
    /// The sum of the usage of the turn's model calls.
    usage: Option<TokenUsage>,
    /// The usage of the last model call that reported one.
    last_call: Option<TokenUsage>,
    /// The usage of the current model call, until the turn counts it.
    call_usage: Option<TokenUsage>,
    /// The context so far: the last real count and what came after it.
    meter: Meter,
    /// The newest estimate of a request.
    estimated: Option<u64>,
    /// The compactions in a row that left the context at or above the trigger, or freed
    /// nothing, in this turn and the turns before it; at
    /// [`BREAKER_TRIES`](crate::BREAKER_TRIES) the turn stops compacting until the
    /// context is below the trigger again.
    misses: u32,
    /// The fresh context block of the newest compaction: the actor's, or the one this
    /// turn read or made.
    fresh: Option<Fresh>,
    /// Where the hidden shell is by the log, for the fresh block when the toolbox cannot
    /// tell.
    logged_shell_cwd: Option<PathBuf>,
    /// The user messages of the conversation so far, for the record of an exit.
    user_messages: Vec<String>,
    /// The exits, refusals and exports of the turn so far.
    exits: TurnExits,
    /// Set when the turn must end after the current call, such as after three
    /// refusals in a row.
    stop: Option<ErrorBody>,
    /// The earlier turns that the newest request carried; see [`TurnEnd::carried`].
    carried: Option<HashSet<TurnId>>,
    /// Why the newest model call that completed stopped, as its provider said.
    answer_stop: Option<StopReason>,
    /// Why the model stopped, for `turn_completed`, when its final answer had no text
    /// and no call or stopped before its end ([`turn_stop`]).
    final_stop: Option<TurnStop>,
    /// The background jobs that the turn's calls started.
    jobs: Vec<JobInfo>,
    /// How many tool calls the turn judged, for the limit of a wake turn.
    calls: usize,
    /// True once the turn recorded the `job_notice` events of its notices.
    notices_recorded: bool,
}

/// What the check point found about one call before the call's start is recorded.
enum Judged {
    /// The toolbox could not say what the call needs; the model reads the text.
    Refused(String),
    /// The engine decided.
    Ruled(Box<Ruling>),
}

/// The engine's decision about one call, and how the call would run.
struct Ruling {
    requirements: Requirements,
    decision: Decision,
    /// How the call runs once it may: [`Launch::Direct`], or for a shell call of the
    /// `auto` mode the narrowest launch that covers its exits.
    launch: Launch,
    /// Why the line may not run outside the sandbox, for a call whose launch would be
    /// the exit child: it gets no question.
    problem: Option<String>,
    /// The engine that decided, for where each path of the record stands.
    engine: Arc<Engine>,
    /// Who approves the call's exits once the user's messages are known; `None`
    /// without exits.
    approver: Option<Approver>,
    /// What a file tool call writes to a floor file, for the record.
    payload: Option<exit::Payload>,
    /// What the question says, when it is not the summary of the call's decision.
    summary: Option<String>,
}

impl Ruling {
    /// How the call starts, for `tool_call_started`: the launch of a call that goes
    /// through the sandbox's launcher and may run or be asked about; `None` otherwise.
    fn planned_launch(&self) -> Option<Launch> {
        let refused = self.decision.effect() == Effect::Deny || self.problem.is_some();
        (!refused && self.launch.uses_launcher()).then(|| self.launch.clone())
    }
}

/// What a quarantine question ended with.
enum Kept {
    /// The user answered: true to keep the changes.
    Answer(bool),
    /// Nobody answered before the timeout or the interrupt, or the answer was lost.
    Unanswered,
}

impl Turn {
    fn new(shared: Arc<Shared>, spec: TurnSpec, control: Control, fresh: Option<Fresh>) -> Self {
        let cwd = shared.deps.home.path().to_path_buf();
        let config = shared.config.current();
        let scratch = config.scratch_root.clone();
        let drafter = Drafter::new(
            shared.deps.drafts.clone(),
            Arc::clone(&shared.status),
            shared.conversation_id,
            spec.turn_id,
            config.draft_interval,
        );
        let misses = shared.misses.load(Ordering::Acquire);
        // NOTE: a turn that notices started goes on with the denials of the one before,
        // so a model with nobody there cannot collect a fresh count each time (phase C).
        let mut exits = spec.carried.clone().unwrap_or_default();
        exits.wake_turn = spec.is_wake();
        Turn {
            shared,
            config,
            settings: None,
            edit_tool: EditTool::default(),
            spec,
            control,
            cwd,
            scope: Scope::Machine,
            scratch,
            transcript: Vec::new(),
            assistant_index: 0,
            streamed: 0,
            drafter,
            usage: None,
            last_call: None,
            call_usage: None,
            meter: Meter::default(),
            estimated: None,
            misses,
            fresh,
            logged_shell_cwd: None,
            user_messages: Vec::new(),
            exits,
            stop: None,
            carried: None,
            answer_stop: None,
            final_stop: None,
            jobs: Vec::new(),
            calls: 0,
            notices_recorded: false,
        }
    }

    fn turn_id(&self) -> TurnId {
        self.spec.turn_id
    }

    async fn drive(
        &mut self,
        cache: &HashMap<TurnId, Arc<CachedTurn>>,
    ) -> Result<Ending, ConversationError> {
        let shared = Arc::clone(&self.shared);
        let config = Arc::clone(&self.config);
        let turn_id = self.turn_id();
        let snapshot =
            Snapshot::read(&shared.deps.readers, shared.conversation_id, config.history).await?;
        let summary = snapshot.summary.as_ref();
        self.cwd = match (&self.spec.context, summary.and_then(|summary| summary.cwd.as_ref())) {
            (Some(context), _) => context.pwd.clone(),
            (None, Some(cwd)) => cwd.clone(),
            (None, None) => shared.deps.home.path().to_path_buf(),
        };
        // NOTE: a wake turn works where the job's turn worked, not where the user went
        // since; it has no context, so the hidden shell does not move.
        if let TurnKind::Wake { cwd } = &self.spec.kind {
            self.cwd = cwd.clone();
        }
        let derivation = shared.deps.scope.resolve(&self.cwd).await;
        self.scope = derivation.scope.clone();
        // NOTE: resolved again against the settings of this moment, because the config
        // may have changed while the prompt waited; what no longer fits fails the turn.
        // `auto` needs the sandbox and a project below the home directory, and a wake
        // turn runs with at most `auto`.
        let engine = Arc::clone(&shared.deps.engine.borrow());
        let sandbox = shared.deps.sandbox.borrow().clone();
        let project_root = match &self.scope {
            Scope::Project(id) => engine.locations().project_root(id),
            _ => None,
        };
        let place = Place {
            sandbox: &sandbox,
            project_root,
            home: engine.locations().home(),
            wake: self.spec.is_wake(),
        };
        let settings =
            match settings::resolve(&self.spec.settings, &config, self.spec.origin, place) {
                Ok(settings) => settings,
                Err(error) => return Ok(Ending::Failed(settings::failure(&error))),
            };
        self.settings = Some(settings.clone());
        self.meter = Meter::new(context::context_base(&snapshot.page, &settings.model));
        self.user_messages = if self.spec.is_wake() {
            exit::wake_user_messages(&snapshot.user_events, &exit::waiting_turns(&snapshot.turns))
        } else {
            exit::user_messages(&snapshot.user_events, turn_id)
        };
        // NOTE: the notices go before `turn_started` in its batch: the first notice of a
        // wake turn makes the turn's row, which `turn_started` then updates.
        let woke = self.spec.is_wake();
        let mut started: Vec<Event> = self
            .spec
            .notices
            .iter()
            .map(|notice| Event::JobNotice {
                conversation_id: shared.conversation_id,
                turn_id,
                job_id: notice.job.id.clone(),
                notice_kind: notice.kind,
                woke,
                source_seq: notice.source_seq,
                not_user_input: true,
                text: notice.text.clone(),
            })
            .collect();
        started.push(Event::TurnStarted {
            turn_id,
            cwd: self.cwd.clone(),
            scope: self.scope.clone(),
            settings: Some(settings.clone()),
        });
        if let Some(previous) = summary.and_then(|summary| summary.scope.clone())
            && previous != self.scope
        {
            started.push(Event::ScopeChanged { turn_id, from: previous, to: self.scope.clone() });
        }
        self.record(started).await?;
        self.notices_recorded = true;
        self.transcript.push(Message::user(self.spec.text.clone()));

        self.scratch = match self.ensure_scratch(&snapshot).await {
            Ok(scratch) => scratch,
            Err(error) => {
                tracing::error!(error = %error, "the scratch directory could not be prepared");
                return Ok(Ending::Failed(ErrorBody::new(ErrorCode::Internal, error.to_string())));
            }
        };

        // NOTE: a `cd` of the user between two prompts moves the hidden shell too, so
        // the model works where the user now is. Without such a move the shell stays
        // where the model left it.
        let moved_from = match snapshot.previous_cwd(turn_id) {
            Some(before) if self.spec.context.is_some() && before != self.cwd => {
                Some(before.to_path_buf())
            }
            _ => None,
        };
        if moved_from.is_some() {
            shared.deps.toolbox.move_shell(shared.conversation_id, &self.cwd).await;
        }
        self.logged_shell_cwd = snapshot.agent_cwd();
        let agent_cwd = match shared.deps.toolbox.shell_cwd(shared.conversation_id).await {
            Some(cwd) => Some(cwd),
            None => snapshot.agent_cwd(),
        };
        let preamble = self.live_state(derivation.repo, agent_cwd, moved_from).render();
        let fresh = match snapshot.summary() {
            Some(compaction) => Some(self.fresh_for(compaction).await),
            None => None,
        };
        // NOTE: the safety net of the history scales with the model's window, so a
        // compaction, not the net, makes room.
        let history = config.history.for_window(self.limits().window);
        let mut window =
            snapshot.window(Some(turn_id), cache, &self.model_key(), history, fresh.as_deref());
        self.carry(&window);
        // NOTE: a wake turn has no prompt: its first message is the preamble and the
        // notices. A prompt turn reads the notices that waited after its prompt.
        let mut blocks = vec![ContentBlock::Text { text: preamble }];
        if !self.spec.is_wake() {
            blocks.push(ContentBlock::Text { text: self.spec.text.clone() });
        }
        blocks.extend(
            self.spec.notices.iter().map(|notice| ContentBlock::Text { text: notice.text.clone() }),
        );
        if self.spec.more_notices > 0 {
            blocks.push(ContentBlock::Text { text: wake::more_notices(self.spec.more_notices) });
        }
        let prompt = Message::new(Role::User, blocks);
        // NOTE: the history is in the base already, the context at the end of the
        // previous turn; only the new prompt comes after it.
        self.meter.add(&prompt);
        // NOTE: the prompt is kept as the model reads it, with its preamble, so every
        // later request sends the same bytes and the provider's cache reads them.
        if let Some(first) = self.transcript.first_mut() {
            first.clone_from(&prompt);
        }
        window.placed.push(Placed { turn: turn_id, index: 0, message: prompt });
        self.edit_tool = edit_tool(&*shared.deps.provider, &settings.model);
        let base = Request {
            model: settings.model.clone(),
            system: config.system_prompt.clone().filter(|system| !system.is_empty()),
            messages: Vec::new(),
            tools: shared.deps.toolbox.definitions(self.edit_tool),
            max_output_tokens: config.max_output_tokens,
            effort: settings.effort.clone(),
            side_call: false,
            output: None,
            one_shot: false,
            provider_options: provider_options(&config, shared.conversation_id),
        };

        for _ in 0..config.max_model_calls {
            if self.control.interrupt.is_raised() {
                return Ok(Ending::Interrupted);
            }
            let steers = self.control.steering.take();
            if !steers.is_empty() {
                // NOTE: recorded before the call that sends them, so a client moves
                // them into the conversation, and an interrupt no longer sends them
                // again as a new prompt.
                let seqs = steers.iter().map(|steer| steer.seq).collect();
                self.record(vec![Event::SteeringDelivered { turn_id, steers: seqs }]).await?;
            }
            for steer in steers {
                self.user_messages.push(steer.text.clone());
                self.push(&mut window, Message::user(steer.text));
            }
            let estimate = match self.guard(&mut window, &base).await? {
                Guard::Send { estimate } => estimate,
                Guard::Full(error) => return Ok(Ending::Failed(error)),
                Guard::Interrupted => return Ok(Ending::Interrupted),
            };
            let message = match self.respond(with_window(&base, &window)).await? {
                Response::Done(message) => message,
                // NOTE: never retried as a transient error: the same request fails
                // again. One compaction and one more try; a second refusal ends the
                // turn.
                Response::Failed(error) if error.is_context_overflow() => {
                    match self.recover(&mut window, &base, estimate).await? {
                        Recovered::Done(message) => message,
                        Recovered::Stop(ending) => return Ok(ending),
                    }
                }
                Response::Failed(error) => return Ok(Ending::Failed(provider_failure(&error))),
                Response::Interrupted => return Ok(Ending::Interrupted),
            };
            let calls = tool_calls(&message);
            let stop = turn_stop(self.answer_stop, &message);
            self.push(&mut window, message);
            self.count_call();
            if calls.is_empty() {
                if !self.control.steering.close_if_idle().await {
                    continue;
                }
                self.final_stop = stop;
                return Ok(Ending::Completed);
            }
            let mut results = Vec::with_capacity(calls.len());
            let mut interrupted = false;
            for call in calls {
                let (result, stopped) = self.tool_call(call, interrupted).await?;
                interrupted |= stopped;
                results.push(result);
                if self.stop.is_some() {
                    break;
                }
            }
            self.push(&mut window, Message::new(Role::User, results));
            if let Some(error) = self.stop.take() {
                return Ok(Ending::Failed(error));
            }
            if interrupted {
                return Ok(Ending::Interrupted);
            }
            if self.capped() {
                // NOTE: nobody asked for a wake turn's steps now, so it does not go on
                // for ever; the next request says why it stopped.
                tracing::info!(calls = self.calls, "the job notice turn reached its limit");
                self.push(&mut window, Message::user(wake::CALL_CAP));
                return Ok(Ending::Completed);
            }
        }
        let limit = config.max_model_calls;
        Ok(Ending::Failed(ErrorBody::new(
            ErrorCode::Internal,
            format!("the turn stopped after {limit} model calls without a final answer"),
        )))
    }

    /// Builds the batch of the terminal event, with the turn's exact messages for the
    /// history of later turns, and hands both to the actor, which records the batch. A
    /// turn that stopped between a tool call and its result (a store error, say) leaves
    /// a call without a result, which later turns could not send; it gets an error
    /// result.
    async fn finish(mut self, ending: Ending) -> TurnEnd {
        let turn_id = self.turn_id();
        let conversation_id = self.shared.conversation_id;
        // NOTE: no model call reads steering from here on, so a steer is late.
        self.control.steering.close();
        // NOTE: one line for a turn that did not complete, so the journal says why a
        // turn shows nothing; the provider's line before it names the request.
        match &ending {
            Ending::Failed(error) => {
                tracing::info!(code = %error.code, message = %error.message, "the turn failed");
            }
            Ending::Interrupted => tracing::info!("the user interrupted the turn"),
            Ending::Completed => {}
        }
        self.shared.misses.store(self.misses, Ordering::Release);
        // NOTE: asked for every ending, so the toolbox keeps the last snapshot of an
        // interrupted or failed turn too, and every ending reports what changed and
        // what `efr undo` can take back.
        let watch = Stopwatch::start();
        let TurnChanges { changes, undo, snapshots } =
            self.shared.deps.toolbox.turn_changes(conversation_id, turn_id).await;
        tracing::debug!(phase = "turn_changes", elapsed_ms = %watch, "phase=turn_changes elapsed_ms={}", watch);
        let usage = self.usage.map(|sum| {
            let mut usage = efr_protocol::Usage::from(sum);
            usage.context_tokens = self.last_call.map_or(0, |call| context_tokens(&call));
            usage
        });
        close_open_calls(&mut self.transcript);
        // NOTE: a turn that did not complete ends with a marker, after the results of
        // the calls that it closed, so the next turn does not answer its prompt. It goes
        // through the meter: the next request carries it.
        if !self.transcript.is_empty()
            && let Some(marker) = end_marker(ending.status())
        {
            self.meter.add(&marker);
            self.transcript.push(marker);
        }
        // NOTE: the last real count plus what came after it, such as the results of a
        // call that an interrupt stopped; after a compaction with no call since, the
        // estimate of the compacted request.
        let tokens = self.meter.known().or(self.estimated);
        let context = tokens.map(|tokens| self.limits().gauge(tokens));
        let event = match ending {
            Ending::Completed => Event::TurnCompleted {
                turn_id,
                usage,
                context,
                changes,
                stop: self.final_stop,
                undo,
            },
            Ending::Failed(error) => {
                Event::TurnFailed { turn_id, error, usage, context, changes, undo }
            }
            Ending::Interrupted => {
                Event::TurnInterrupted { turn_id, usage, context, changes, undo }
            }
        };
        let key = self.model_key();
        let mut batch = Batch::new();
        // NOTE: the snapshots and the report come before the terminal event, so a view
        // that stops at the end of the turn has shown them. A snapshot from before the
        // turn's first write is sent here too: a file tool takes it without a word.
        for snapshot in snapshots {
            let event = Event::SnapshotTaken {
                turn_id,
                root: snapshot.root,
                phase: snapshot.phase,
                commit: snapshot.commit,
                files: snapshot.files,
                skipped: snapshot.skipped,
                ms: snapshot.ms,
            };
            batch = batch.event(conversation_id, event);
        }
        let files = self.shared.deps.toolbox.turn_report(conversation_id, turn_id).await;
        if !files.is_empty() {
            batch = batch.event(conversation_id, Event::TurnSurfaceReport { turn_id, files });
        }
        batch = batch.event(conversation_id, event);
        if let Some(messages) = self.saved_messages(&key) {
            batch = batch.turn_messages(messages);
        }
        let cached =
            (!self.transcript.is_empty()).then_some(CachedTurn { key, messages: self.transcript });
        let untaken = if self.notices_recorded { Vec::new() } else { self.spec.notices };
        TurnEnd {
            turn_id,
            cached,
            record: batch,
            fresh: self.fresh,
            carried: self.carried,
            jobs: self.jobs,
            mode: self.settings.map(|settings| settings.mode),
            cwd: self.cwd,
            exits: self.exits,
            untaken,
        }
    }

    /// The turn's messages as the store saves them, or `None` for a turn that never
    /// started.
    fn saved_messages(&self, key: &ModelKey) -> Option<NewTurnMessages> {
        if self.transcript.is_empty() {
            return None;
        }
        let messages = self.transcript.iter().map(serde_json::to_value).collect();
        let messages = match messages {
            Ok(messages) => messages,
            Err(error) => {
                // NOTE: the turn's end matters more than its provider items; a later turn
                // rebuilds this one from its events.
                tracing::warn!(error = %error, "the turn's messages could not be saved");
                return None;
            }
        };
        Some(NewTurnMessages::new(
            self.shared.conversation_id,
            self.turn_id(),
            key.provider.as_str(),
            key.model.as_str(),
            messages,
        ))
    }

    /// The provider and the model that answer this turn.
    fn model_key(&self) -> ModelKey {
        let model = self.settings.as_ref().map_or(&self.config.model, |settings| &settings.model);
        ModelKey::new(self.shared.deps.provider.id().clone(), model.clone())
    }

    /// True when nobody can answer a question of the turn now: the user is away, or,
    /// in a turn that job notices started, no client that can answer follows the
    /// conversation. Nobody typed the input of such a turn, so a user who typed in
    /// another terminal a while ago does not count as there (phase C).
    fn nobody_there(&self) -> bool {
        let presence = &self.shared.deps.presence;
        let conversation_id = self.shared.conversation_id;
        presence.presence(conversation_id) == Presence::Away
            || (self.spec.is_wake() && !presence.followed(conversation_id))
    }

    /// True when the turn is a wake turn that judged [`wake::WAKE_TURN_MAX_CALLS`]
    /// calls: no more calls run in it.
    fn capped(&self) -> bool {
        self.spec.is_wake() && self.calls >= wake::WAKE_TURN_MAX_CALLS
    }

    /// The turn's permission mode; the config's until the turn has resolved its own.
    fn mode(&self) -> Mode {
        self.settings.as_ref().map_or(self.config.mode, |settings| settings.mode)
    }

    /// Adds `message` to the request's window and to the turn's transcript, where its
    /// place is.
    fn push(&mut self, window: &mut Window, message: Message) {
        self.meter.add(&message);
        let index = u32::try_from(self.transcript.len()).unwrap_or(u32::MAX);
        window.placed.push(Placed { turn: self.turn_id(), index, message: message.clone() });
        self.transcript.push(message);
    }

    /// Notes the earlier turns that `window` carries, the only ones that the next turn
    /// can need from the actor's cache.
    fn carry(&mut self, window: &Window) {
        let turn_id = self.turn_id();
        let earlier =
            window.placed.iter().map(|placed| placed.turn).filter(|turn| *turn != turn_id);
        self.carried = Some(earlier.collect());
    }

    /// Adds to the window the messages of the transcript that it lacks: the text that
    /// streamed before a refused call, which the user saw and the transcript keeps, so
    /// the place of every later message stays that of the transcript.
    fn catch_up(&self, window: &mut Window) {
        let turn_id = self.turn_id();
        let next = window
            .placed
            .iter()
            .rev()
            .find(|placed| placed.turn == turn_id)
            .map_or(0, |placed| placed.index as usize + 1);
        for (index, message) in self.transcript.iter().enumerate().skip(next) {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            window.placed.push(Placed { turn: turn_id, index, message: message.clone() });
        }
    }

    fn live_state(
        &self,
        repo: Option<efr_scope::Repo>,
        agent_cwd: Option<PathBuf>,
        moved_from: Option<PathBuf>,
    ) -> LiveState {
        let context = self.spec.context.as_ref();
        let host = &self.config.host;
        LiveState {
            cwd: self.cwd.clone(),
            oldpwd: context.and_then(|context| context.oldpwd.clone()),
            last_command: self.spec.last_command.clone(),
            last_status: context.and_then(|context| context.last_status),
            repo,
            home: self.shared.deps.home.path().to_path_buf(),
            host: context
                .and_then(|context| context.hostname.clone())
                .or_else(|| host.hostname.clone()),
            os: host.os.clone(),
            ssh: context.is_some_and(|context| context.ssh_connection.is_some()),
            scratch: self.scratch.clone(),
            agent_cwd,
            moved_from,
            mode: self.mode(),
            fallback: self.settings.as_ref().and_then(|settings| settings.fallback.clone()),
            model: self.model_key().model,
            effort: self
                .settings
                .as_ref()
                .map_or_else(|| self.config.effort.clone(), |settings| settings.effort.clone()),
            egress: self.shared.deps.engine.borrow().support().egress,
            full_allowed: self.config.full_mode.allowed,
        }
    }

    /// The conversation's `$SCRATCH`, named after the day it began and its title.
    async fn ensure_scratch(&self, snapshot: &Snapshot) -> Result<PathBuf, ConversationError> {
        let summary = snapshot.summary.as_ref();
        let began = summary.map_or_else(|| self.shared.deps.clock.now(), |s| s.created_at);
        let began = began.to_zoned(self.config.time_zone.clone()).date();
        let title = summary.and_then(|summary| summary.title.clone()).or_else(|| {
            self.spec.text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned)
        });
        let shared = Arc::clone(&self.shared);
        tokio::task::spawn_blocking(move || {
            let mut scratch = shared.scratch.lock().unwrap_or_else(PoisonError::into_inner);
            scratch.ensure(began, title.as_deref())
        })
        .await
        .map_err(|_| ConversationError::TaskPanicked { task: "scratch" })?
    }

    /// Records, runs and answers one tool call. With `skip`, the call is recorded as not
    /// run, because an earlier call of the same answer was interrupted. The second value
    /// is true when the user interrupted the turn during this call.
    async fn tool_call(
        &mut self,
        call: PendingCall,
        skip: bool,
    ) -> Result<(ContentBlock, bool), ConversationError> {
        let turn_id = self.turn_id();
        let call_id = CallId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng));
        let span = tracing::info_span!("tool_call", tool = %call.name, call_id = %call_id);
        async move {
            // NOTE: the `phase` lines of a call (docs/sandbox.md): each says how long one
            // step took, so `EFR_LOG=debug` shows where the time of a call goes.
            let call_watch = Stopwatch::start();
            let manual_input = self.shared.deps.toolbox.takes_manual_input(&call.name, &call.input);
            // NOTE: asked for every call, because the call before may have moved the
            // hidden shell, and a command's relative paths run from where it is now.
            let shell_cwd = self.shared.deps.toolbox.shell_cwd(self.shared.conversation_id).await;
            let context = CallContext {
                conversation_id: self.shared.conversation_id,
                turn_id,
                call_id,
                cwd: self.cwd.clone(),
                shell_cwd,
                scratch: self.scratch.clone(),
                scope: self.scope.clone(),
                origin: self.spec.origin,
                approved_interactive: false,
                launch: Launch::Direct,
                exits: Vec::new(),
                mode: efr_permissions::effective_mode(self.mode(), self.spec.origin),
                edit_tool: self.edit_tool,
            };
            let mut tool_call = ToolCall::new(call.name, call.input, context);
            let interrupt = skip || self.control.interrupt.is_raised();
            let capped = !interrupt && self.capped();
            let skipped = interrupt || capped;
            if !skipped {
                self.calls += 1;
            }
            // NOTE: checked before the engine, so a call of a full turn whose gate went
            // off is never judged and never runs (DF16).
            let gate = if skipped { None } else { self.full_gate(&tool_call.context) };
            // NOTE: judged before the start is recorded, so `tool_call_started` says how
            // the call runs; the engine is pure, and nothing runs until it allows.
            let judged = if skipped || gate.is_some() {
                None
            } else {
                Some(self.judge(&tool_call).await)
            };
            let launch = match &judged {
                Some(Judged::Ruled(ruling)) => ruling.planned_launch(),
                _ => None,
            };
            // NOTE: what the engine dropped from the call's `needs`, such as a host that
            // the proxy's allow list holds (efr's auto spec, 7.3); the model reads it with
            // the result.
            let notes: Vec<String> = match &judged {
                Some(Judged::Ruled(ruling)) => ruling.decision.notes().to_vec(),
                _ => Vec::new(),
            };
            let watch = Stopwatch::start();
            self.record(vec![Event::ToolCallStarted {
                turn_id,
                call_id,
                tool: tool_call.name.clone(),
                input: tool_call.input.clone(),
                manual_input,
                launch,
                freeform: call.freeform,
            }])
            .await?;
            tracing::debug!(phase = "record_started", elapsed_ms = %watch, "phase=record_started elapsed_ms={}", watch);
            let mut refused = None;
            let (mut outcome, mut interrupted) = match judged {
                None if capped => (ToolOutcome::error(wake::CALL_CAP), false),
                None => match gate {
                    Some(message) => {
                        self.stop_for_full(&message);
                        refused = Some(approvals::brief(&message));
                        (ToolOutcome::error(message), false)
                    }
                    None => (ToolOutcome::error(NOT_RUN), true),
                },
                Some(judged) => match self.authorize_tool_call(&tool_call, judged).await? {
                    (Authorization::Allowed { approved_interactive, launch }, exits) => {
                        tool_call.context.approved_interactive = approved_interactive;
                        tool_call.context.launch = launch;
                        tool_call.context.exits = exits;
                        let watch = Stopwatch::start();
                        let invoked = self.invoke(tool_call.clone()).await?;
                        tracing::debug!(phase = "tool_run", elapsed_ms = %watch, "phase=tool_run elapsed_ms={}", watch);
                        // NOTE: efrd recorded `job_started` before the call answered; the
                        // actor keeps how this turn ran for the job's notices.
                        if let Some(job) = invoked.as_ref().and_then(|outcome| outcome.job.clone()) {
                            self.jobs.push(job);
                        }
                        match invoked {
                            Some(outcome) => (outcome, false),
                            None => (ToolOutcome::error(STOPPED), true),
                        }
                    }
                    (
                        Authorization::Denied { message, refusal }
                        | Authorization::Refused { message, refusal },
                        _,
                    ) => {
                        refused = refusal;
                        (ToolOutcome::error(message), false)
                    }
                    (Authorization::Interrupted, _) => (ToolOutcome::error(NOT_RUN), true),
                    (Authorization::Expired { away: false }, _) => {
                        (ToolOutcome::error(EXPIRED), false)
                    }
                    (Authorization::Expired { away: true }, _) => {
                        (ToolOutcome::error(exit::AWAY_EXPIRED), false)
                    }
                },
            };
            let watch = Stopwatch::start();
            let mut events = vec![Event::ToolCallCompleted {
                turn_id,
                call_id,
                output: outcome.output.clone(),
                truncated: outcome.truncated,
                is_error: outcome.is_error,
                exit_code: outcome.exit_code,
                sandbox: outcome.sandbox.clone(),
                refusal: refused,
                changes: outcome.changes.take(),
                diff: outcome.diff.take(),
            }];
            // NOTE: names only: the copy stays in efr's store, and the line at the end of
            // the turn names the file.
            events.extend(
                std::mem::take(&mut outcome.backups)
                    .into_iter()
                    .map(|backup| home_file_changed(turn_id, call_id, backup)),
            );
            let mut quarantined = Vec::new();
            if let Some(summary) = &outcome.sandbox {
                self.exits.ran(summary);
                if !summary.surface_changes.is_empty() {
                    quarantined = exit::quarantined(summary);
                    events.push(Event::SandboxSurfaceChanged {
                        turn_id,
                        call_id,
                        changes: summary.surface_changes.clone(),
                        quarantined: !quarantined.is_empty(),
                    });
                }
            }
            self.record(events).await?;
            tracing::debug!(phase = "record_completed", elapsed_ms = %watch, "phase=record_completed elapsed_ms={}", watch);
            if !quarantined.is_empty() {
                // NOTE: asked before any other call of the turn, so nothing runs with a
                // git setting that the sandbox planted until the user has seen it.
                let (note, stopped) = self.ask_surface(&tool_call.context, quarantined).await?;
                outcome.output = format!("{}\n{note}", outcome.output);
                interrupted |= stopped;
            }
            if let Some(summary) = outcome.sandbox.clone()
                && !summary.cache_deletes.is_empty()
                && !interrupted
            {
                let (note, stopped) = self.cache_deletes(&tool_call, &summary).await?;
                if let Some(note) = note {
                    outcome.output = format!("{}\n{note}", outcome.output);
                }
                interrupted |= stopped;
            }
            for note in notes {
                outcome.output = format!("{}\n{note}", outcome.output);
            }
            let result = ContentBlock::ToolResult {
                call_id: call.provider_call_id,
                output: outcome.output,
                is_error: outcome.is_error,
            };
            tracing::debug!(phase = "tool_call", elapsed_ms = %call_watch, "phase=tool_call elapsed_ms={}", call_watch);
            Ok((result, interrupted))
        }
        .instrument(span)
        .await
    }

    /// What the toolbox declares that `call` needs, and the engine's decision about it,
    /// with the turn's scope, origin, mode and the conversation's policy. Pure: nothing
    /// is recorded and nothing runs.
    async fn judge(&self, call: &ToolCall) -> Judged {
        let watch = Stopwatch::start();
        let requirements = match self.shared.deps.toolbox.requirements(call).await {
            Ok(requirements) => requirements,
            Err(message) => return Judged::Refused(message),
        };
        tracing::debug!(phase = "requirements", elapsed_ms = %watch, "phase=requirements elapsed_ms={}", watch);
        let input = DecisionInput {
            requirements,
            scope: call.context.scope.clone(),
            origin: call.context.origin,
            mode: self.mode(),
            conversation_policy: ConversationPolicy::new(&call.context.scratch)
                .with_rules(self.config.policy.clone()),
        };
        // NOTE: the engine is cloned out so the watch's read lock is not held while
        // the turn records events or waits for an answer.
        let engine = Arc::clone(&self.shared.deps.engine.borrow());
        let watch = Stopwatch::start();
        let decision = engine.decide(&input);
        tracing::debug!(effect = %decision.effect(), "the permission engine decided");
        tracing::debug!(phase = "engine", elapsed_ms = %watch, "phase=engine elapsed_ms={}", watch);
        let watch = Stopwatch::start();
        let requirements = input.requirements;
        // NOTE: only a shell call of a local `auto` turn runs in the sandbox; the file
        // tools run in the daemon, and a remote turn never runs as `auto`.
        let sandboxed = efr_permissions::effective_mode(input.mode, input.origin) == Mode::Auto
            && requirements.command.is_some();
        let launch = if sandboxed { exit::grant(&decision) } else { Launch::Direct };
        let problem = match (&launch, decision.effect(), &requirements.command) {
            (Launch::Unsandboxed, Effect::Ask, Some(line)) => {
                efr_permissions::exits::unsandboxed_line_problem(line)
            }
            _ => None,
        };
        let payload = exit::payload(
            &call.name,
            &call.input,
            &decision,
            engine.locations().home(),
            &call.context.cwd,
        );
        let approver = exit::route(&decision, &self.user_messages, payload.as_ref());
        tracing::debug!(phase = "exit_prediction", elapsed_ms = %watch, "phase=exit_prediction elapsed_ms={}", watch);
        Judged::Ruled(Box::new(Ruling {
            requirements,
            decision,
            launch,
            problem,
            engine,
            approver,
            payload,
            summary: None,
        }))
    }

    /// The single permission check point: may `call` run, and how?
    ///
    /// The toolbox declared what the call needs and [`efr_permissions::Engine::decide`]
    /// judged it ([`Turn::judge`]); the effect is enforced here: `Allow` lets the call
    /// run, `Contain` runs it in the `auto` sandbox with no question, `Deny` gives the
    /// model an error that names each refused path with its class, and `Ask` records
    /// `approval_requested` (after `exit_requested` for an exit) and parks the turn
    /// until the user answers, the turn is interrupted, or the request expires. The
    /// second value is the exits that a "yes" approved.
    async fn authorize_tool_call(
        &mut self,
        call: &ToolCall,
        judged: Judged,
    ) -> Result<(Authorization, Vec<efr_permissions::ExitNeed>), ConversationError> {
        let ruling = match judged {
            Judged::Refused(message) => {
                return Ok((Authorization::Refused { message, refusal: None }, Vec::new()));
            }
            Judged::Ruled(ruling) => ruling,
        };
        let approved: Vec<efr_permissions::ExitNeed> = ruling.decision.exits().cloned().collect();
        let authorization = match ruling.decision.effect() {
            Effect::Allow => {
                // NOTE: only full mode allows a call with exits: they run with no
                // question, and the history keeps what full mode let through.
                if call.context.is_full() && !approved.is_empty() {
                    self.let_through(call, &ruling).await?;
                }
                Authorization::Allowed {
                    approved_interactive: false,
                    launch: ruling.launch.clone(),
                }
            }
            // NOTE: a contained call runs only through the launcher; anything else that
            // the engine contains asks, so nothing runs outside the sandbox without a
            // person.
            Effect::Contain if ruling.launch.uses_launcher() => Authorization::Allowed {
                approved_interactive: false,
                launch: ruling.launch.clone(),
            },
            Effect::Deny => {
                self.refuse_floor(call, &ruling).await?;
                Authorization::Denied {
                    message: approvals::denial(&call.name, &ruling.decision),
                    refusal: Some(approvals::refusal(&ruling.decision)),
                }
            }
            Effect::Contain | Effect::Ask => match &ruling.problem {
                // NOTE: no question and no refusal: the model splits the line.
                Some(problem) => Authorization::Refused {
                    message: problem.clone(),
                    refusal: Some(approvals::brief(problem)),
                },
                // NOTE: after three expired questions about guarded deletes, the turn
                // asks no more: the model cannot park it on questions nobody reads.
                None if self.exits.critical_closed()
                    && ruling.decision.critical().next().is_some() =>
                {
                    self.refuse_critical(&ruling)
                }
                None => self.ask(call, &ruling).await?,
            },
        };
        let exits = match &authorization {
            Authorization::Allowed { .. } => approved,
            _ => Vec::new(),
        };
        Ok((authorization, exits))
    }

    /// Records a floor's refusal of the exits of `ruling`, and stops the turn at the
    /// third refusal in a row. A refusal that no floor made (a user's `deny` rule, a
    /// secret path) is no exit refusal.
    async fn refuse_floor(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
    ) -> Result<(), ConversationError> {
        let floors = exit::floor_kinds(&ruling.decision);
        let Some(first) = floors.first() else {
            return Ok(());
        };
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let record = self.exit_record(call, ruling);
        let events = vec![
            Event::ExitRequested {
                turn_id,
                call_id,
                kinds: exit::kinds(&ruling.decision),
                grants: Vec::new(),
                source: exit::source(&ruling.decision),
                record: Box::new(record),
            },
            Event::ExitJudged {
                turn_id,
                call_id,
                judge: JudgeKind::Floor,
                verdict: Verdict::Deny,
                model: None,
                latency_ms: None,
                risk: None,
                user_authorization: None,
                category: Some(first.as_str().to_owned()),
                rationale: None,
                record_sha256: None,
                cached: false,
                chain_step: None,
                skipped: Vec::new(),
            },
        ];
        self.record(events).await?;
        self.exits.judged(&floors, Verdict::Deny);
        if self.exits.refused() {
            let message = exit::refusals_stopped(self.mode());
            self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, message));
        }
        Ok(())
    }

    /// The record of the exits of `ruling` for `call`.
    fn exit_record(&self, call: &ToolCall, ruling: &Ruling) -> efr_protocol::ExitRecord {
        let cwd = exit::start_dir(
            &ruling.requirements,
            call.context.shell_cwd.as_deref(),
            &call.context.cwd,
        );
        let action = exit::Action {
            tool: &call.name,
            decision: &ruling.decision,
            requirements: &ruling.requirements,
            launch: &ruling.launch,
            cwd: &cwd,
            scope: &call.context.scope,
            scratch: &call.context.scratch,
            engine: &ruling.engine,
            payload: ruling.payload.as_ref(),
        };
        exit::record(action, &self.user_messages, &self.exits)
    }

    /// Parks `call` until the user answers, the turn is interrupted, or the approval
    /// timeout passes. An approval of an `interactive` call, one that may wait for input
    /// at the terminal, says so. A call with exits records their record first, and the
    /// question shows them; the user's answer is recorded as their judgement.
    async fn ask(
        &mut self,
        call: &ToolCall,
        ruling: &Ruling,
    ) -> Result<Authorization, ConversationError> {
        let watch = Stopwatch::start();
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let interactive = ruling.requirements.interactive;
        let decision = &ruling.decision;
        let kinds = exit::kinds(decision);
        let mut events = Vec::with_capacity(2);
        // NOTE: the rule that an `a` answer saves, when the question offers one: only
        // with a writer of the daemon (phase E).
        let mut offer: Option<AllowRule> = None;
        let exit = if kinds.is_empty() {
            None
        } else {
            let record = self.exit_record(call, ruling);
            // NOTE: the always-allow rules come after the floors and the user's rules,
            // which the engine applied, and before the classifier (DE21).
            match self.always(call, ruling) {
                AlwaysVerdict::Covered { index } => {
                    return self.covered(call, ruling, record, index).await;
                }
                AlwaysVerdict::Offer(rule) if self.shared.deps.allow_writer.is_some() => {
                    offer = Some(rule);
                }
                _ => {}
            }
            let (recorded, judgement) = match self.review(call, ruling, &record).await? {
                Reviewed::Decided(authorization) => return Ok(authorization),
                Reviewed::Ask { recorded, judgement } => (recorded, judgement),
            };
            let stopped = self.paused_and_away(call, ruling);
            if !recorded {
                events.push(Event::ExitRequested {
                    turn_id,
                    call_id,
                    kinds: kinds.clone(),
                    grants: ruling.launch.grants().to_vec(),
                    source: exit::source(decision),
                    record: Box::new(record),
                });
            }
            // NOTE: the exit that stops the turn keeps its record, so the history shows
            // what nobody could decide.
            if let Some(stopped) = stopped {
                self.record(events).await?;
                return Ok(stopped);
            }
            let home = ruling.engine.locations().home();
            // NOTE: full mode has no reviewer: only the user answers what it asks.
            let approver = match ruling.approver {
                Some(approver) if !call.context.is_full() => approver,
                _ => Approver::User,
            };
            let mut info =
                exit::info(decision, &ruling.requirements, &ruling.launch, home, approver);
            info.judged = judgement;
            info.always = offer.as_ref().map(AllowRule::describe);
            Some(info)
        };
        let mut summary =
            ruling.summary.clone().unwrap_or_else(|| approvals::summary(&call.name, decision));
        let home = ruling.engine.locations().home();
        // NOTE: a privilege exit, and a program such as sudo that runs as another user in
        // any mode, needs the password at the terminal, so a phone never answers it
        // (efr's auto spec, section 9.5). Nor does it answer a job's question: its "yes"
        // is not one-shot, it lasts while nobody watches the job. Nor a delete that full
        // mode guards: only the user's own machine approves it.
        let privileged = decision
            .reasons()
            .iter()
            .any(|reason| matches!(reason.cause, Cause::Privileged { .. }))
            || ruling.requirements.command.as_deref().is_some_and(runs_as_another_user)
            || kinds.contains(&ExitKind::Privilege);
        let critical: Vec<String> =
            decision.critical().map(|delete| delete.describe(home)).collect();
        let local_only = privileged || !critical.is_empty() || ruling.requirements.background;
        // NOTE: the floor that full mode still holds comes first, so the user reads why
        // a mode with no questions asks.
        if call.context.is_full()
            && let Some(line) = approvals::full_line(decision, home, privileged)
        {
            summary.insert(0, '\n');
            summary.insert_str(0, &line);
        }
        // NOTE: a present user sees that a notice, not the user, led to the question.
        if self.spec.is_wake() {
            summary.insert_str(0, WAKE_QUESTION);
        }
        // NOTE: a job's question names how long the job and what a "yes" opens last; a
        // question with exits names it among their facts.
        if let (Some(minutes), None) = (exit::job_minutes(&ruling.requirements), &exit) {
            summary.push('\n');
            summary.push_str(&crate::job_lifetime_fact(minutes));
        }
        let diff_preview = self.shared.deps.toolbox.preview(call).await;
        // NOTE: read at each call, not at turn start, so a reload reaches the next
        // approval of a running turn. An answer wins during the whole wait.
        let config = self.shared.config.current();
        // NOTE: a question about a guarded delete waits at most `CRITICAL_WAIT`, or the
        // approval timeout when it is shorter, and not at all when nobody is there.
        let deadline = (!critical.is_empty()).then(|| {
            config.approval_timeout.map_or(exit::CRITICAL_WAIT, |t| t.min(exit::CRITICAL_WAIT))
        });
        let at_once = deadline.is_some() && self.nobody_there();
        let answer = self.shared.approvals.park(turn_id, call_id, local_only, offer.clone());
        events.push(Event::ApprovalRequested {
            turn_id,
            call_id,
            summary,
            diff_preview,
            interactive,
            exit,
            expires_in_secs: deadline
                .filter(|_| !at_once)
                .map(|deadline| u32::try_from(deadline.as_secs()).unwrap_or(u32::MAX)),
        });
        if let Err(error) = self.record(events).await {
            self.shared.approvals.withdraw(call_id);
            return Err(error);
        }
        tracing::debug!(phase = "approval_request", elapsed_ms = %watch, "phase=approval_request elapsed_ms={}", watch);
        let interrupt = self.control.interrupt.clone();
        let clock = Arc::clone(&self.shared.deps.clock);
        let presence = Arc::clone(&self.shared.deps.presence);
        let conversation_id = self.shared.conversation_id;
        let parked = clock.now();
        let mut answer = answer;
        // NOTE: presence is read when the question parks and again every
        // `PRESENCE_CHECK` while it waits, so a user who leaves, or says so with
        // `efr away`, after it parked still gets the away wait, from the moment efr
        // sees it. A user who comes back gets the normal wait again.
        let mut away_from: Option<Duration> = None;
        let mut pushed = false;
        let watch = Stopwatch::start();
        let waited = loop {
            let elapsed =
                Duration::try_from(clock.now().duration_since(parked)).unwrap_or_default();
            if self.nobody_there() {
                away_from.get_or_insert(elapsed);
                if !pushed && !at_once {
                    presence.push(conversation_id, call_id);
                    pushed = true;
                }
            } else {
                away_from = None;
            }
            let left = match away_from {
                Some(from) => Some((from + config.away_wait).saturating_sub(elapsed)),
                None => config.approval_timeout.map(|timeout| timeout.saturating_sub(elapsed)),
            };
            // NOTE: the smallest of the deadline of a guarded delete and the other waits.
            let left = match (left, deadline) {
                _ if at_once => Some(Duration::ZERO),
                (Some(left), Some(deadline)) => Some(left.min(deadline.saturating_sub(elapsed))),
                (None, Some(deadline)) => Some(deadline.saturating_sub(elapsed)),
                (left, None) => left,
            };
            if deadline.is_some() && left == Some(Duration::ZERO) {
                break Waited::TimedOut;
            }
            tokio::select! {
                biased;
                () = interrupt.raised() => break Waited::Interrupted,
                received = &mut answer => break Waited::Answer(received.ok()),
                () = sleep_or_pending(&*clock, left) => break Waited::TimedOut,
                () = clock.sleep(PRESENCE_CHECK) => {}
            }
        };
        let away = away_from.is_some();
        tracing::debug!(phase = "approval_wait", elapsed_ms = %watch, "phase=approval_wait elapsed_ms={}", watch);
        // NOTE: the actor takes an `a` only for a question that offers a rule; any
        // other `a` falls to the denial below.
        let saved = match waited {
            Waited::Answer(Some(ApprovalDecision::AllowAlways)) => offer,
            _ => None,
        };
        let authorization = match waited {
            Waited::Answer(Some(ApprovalDecision::Allow)) => Authorization::Allowed {
                approved_interactive: interactive,
                launch: ruling.launch.clone(),
            },
            Waited::Answer(Some(ApprovalDecision::AllowAlways)) if saved.is_some() => {
                Authorization::Allowed {
                    approved_interactive: interactive,
                    launch: ruling.launch.clone(),
                }
            }
            // NOTE: a decision added to the protocol later denies, so a newer client
            // can never run a call that this build would not.
            Waited::Answer(Some(_)) if !kinds.is_empty() => {
                Authorization::Denied { message: exit::EXIT_DENIED.to_owned(), refusal: None }
            }
            Waited::Answer(Some(_)) => {
                Authorization::Denied { message: user_denied(&call.name), refusal: None }
            }
            Waited::Answer(None) => return Ok(Authorization::Expired { away: false }),
            Waited::Interrupted => {
                self.expire(call_id, false, &kinds, false).await?;
                return Ok(Authorization::Interrupted);
            }
            Waited::TimedOut => {
                let full = call.context.is_full();
                let expired = self.expire(call_id, away, &kinds, full).await?;
                if !critical.is_empty() && expired {
                    let waited = (!at_once).then(|| {
                        Duration::try_from(clock.now().duration_since(parked)).unwrap_or_default()
                    });
                    return Ok(self.critical_expired(&critical, waited));
                }
                return Ok(Authorization::Expired { away });
            }
        };
        // NOTE: a person answered, so the refusals without one start again.
        self.exits.answered();
        if !kinds.is_empty() {
            let verdict = match authorization {
                Authorization::Allowed { .. } => Verdict::Allow,
                _ => Verdict::Deny,
            };
            self.exits.judged(&kinds, verdict);
            self.record(vec![Event::ExitJudged {
                turn_id,
                call_id,
                judge: JudgeKind::User,
                verdict,
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
            }])
            .await?;
        }
        // NOTE: the call runs as a "yes" whatever the writer says; the rule counts from
        // the next call on (DE26).
        if let Some(rule) = saved {
            self.save_always(call, rule).await?;
        }
        Ok(authorization)
    }

    /// Records that the approval of `call_id` can no longer be answered, unless an
    /// answer took it first; true when it recorded so. `away` when nobody answered while
    /// the user was away: a question about exits `kinds` then records that nobody judged
    /// them. `full` for a question of full mode that timed out: its exits record the
    /// judge `full` with the category `expired`.
    async fn expire(
        &self,
        call_id: CallId,
        away: bool,
        kinds: &[ExitKind],
        full: bool,
    ) -> Result<bool, ConversationError> {
        if self.shared.approvals.withdraw(call_id) {
            let turn_id = self.turn_id();
            let mut events = vec![Event::ApprovalExpired { turn_id, call_id, away }];
            let judge = if full {
                Some(JudgeKind::Full)
            } else {
                away.then_some(JudgeKind::PresenceExpired)
            };
            if let Some(judge) = judge.filter(|_| !kinds.is_empty()) {
                events.push(Event::ExitJudged {
                    turn_id,
                    call_id,
                    judge,
                    verdict: Verdict::Deny,
                    model: None,
                    latency_ms: None,
                    risk: None,
                    user_authorization: None,
                    category: full.then(|| "expired".to_owned()),
                    rationale: None,
                    record_sha256: None,
                    cached: false,
                    chain_step: None,
                    skipped: Vec::new(),
                });
            }
            self.record(events).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Asks the user whether to keep `changes`, which the call of `context` made and the
    /// launcher moved to quarantine, and waits until the user answers, the turn is
    /// interrupted, or the approval timeout passes. Only a "keep" moves them back.
    /// Returns what the model reads about them, and true when the user interrupted the
    /// turn.
    async fn ask_surface(
        &mut self,
        context: &CallContext,
        changes: Vec<SurfaceChange>,
    ) -> Result<(String, bool), ConversationError> {
        let turn_id = self.turn_id();
        let question_id =
            QuestionId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng));
        let mut answer = self.shared.questions.park(turn_id, question_id);
        let requested = Event::SurfaceQuestionRequested {
            turn_id,
            call_id: context.call_id,
            question_id,
            changes: changes.clone(),
        };
        if let Err(error) = self.record(vec![requested]).await {
            self.shared.questions.withdraw(question_id);
            return Err(error);
        }
        let interrupt = self.control.interrupt.clone();
        let clock = Arc::clone(&self.shared.deps.clock);
        let timeout = self.shared.config.current().approval_timeout;
        let waited = tokio::select! {
            biased;
            () = interrupt.raised() => None,
            answer = &mut answer => Some(answer.ok()),
            () = sleep_or_pending(&*clock, timeout) => None,
        };
        let interrupted = interrupt.is_raised();
        let kept = match waited {
            Some(Some(keep)) => Kept::Answer(keep),
            // NOTE: the actor took the question but could not record the answer.
            Some(None) => Kept::Unanswered,
            None if self.shared.questions.withdraw(question_id) => {
                self.record(vec![Event::SurfaceQuestionAnswered {
                    turn_id,
                    question_id,
                    keep: false,
                    origin: None,
                }])
                .await?;
                Kept::Unanswered
            }
            // NOTE: the actor took the question at this moment and records the answer,
            // so the turn waits for it and records none of its own.
            None => answer.await.map_or(Kept::Unanswered, Kept::Answer),
        };
        let names = exit::change_names(&changes);
        let note = match kept {
            Kept::Answer(true) => {
                match self.shared.deps.toolbox.restore_quarantine(context, &changes).await {
                    Ok(()) => {
                        format!("[The user kept the git change; it moved back: {names}.]")
                    }
                    Err(error) => format!(
                        "[The user kept the git change, but it could not move back, so it \
                         stays in quarantine: {names}: {error}]"
                    ),
                }
            }
            Kept::Answer(false) => {
                format!("[The user did not keep the git change; it stays in quarantine: {names}.]")
            }
            Kept::Unanswered => {
                format!("[Nobody answered; the git change stayed in quarantine: {names}.]")
            }
        };
        Ok((note, interrupted))
    }

    /// Runs an authorized call, recording its output as it grows and each change of
    /// whether it waits for input, in the order they happened. `None` when the user
    /// interrupted it: the call's future is dropped and the toolbox asked to stop it.
    async fn invoke(&mut self, call: ToolCall) -> Result<Option<ToolOutcome>, ConversationError> {
        let toolbox: Arc<dyn Toolbox> = Arc::clone(&self.shared.deps.toolbox);
        let clock = Arc::clone(&self.shared.deps.clock);
        let interrupt = self.control.interrupt.clone();
        let context = call.context.clone();
        let (sender, mut output) = watch::channel(None);
        let (inputs, mut waits) = mpsc::channel(INPUT_CAPACITY);
        let mut sink = WatchSink { sender, inputs };
        // NOTE: read at each call, as the approval timeout is.
        let mut updates = Coalescer::new(self.shared.config.current().update_interval);
        let mut invoked = Box::pin(toolbox.invoke(call, &mut sink));
        let outcome = loop {
            let flush = updates.flush_after(clock.now());
            tokio::select! {
                biased;
                () = interrupt.raised() => break None,
                outcome = &mut invoked => break Some(outcome),
                Some(wait) = waits.recv() => {
                    // The sender lives in `sink`, which outlives this loop, so the
                    // channel never closes here.
                    if output.has_changed().unwrap_or(false) {
                        updates.flushed(clock.now());
                    }
                    self.record_input(&context, &mut output, wait).await?;
                }
                changed = output.changed() => {
                    // The sender lives in `sink`, which outlives this loop.
                    if changed.is_ok() && updates.offer(clock.now()) {
                        self.record_output(&context, &mut output).await?;
                    }
                }
                () = sleep_or_pending(&*clock, flush) => {
                    updates.flushed(clock.now());
                    self.record_output(&context, &mut output).await?;
                }
            }
        };
        drop(invoked);
        match outcome {
            // A change the tool made just before it returned is still recorded, before
            // the call's completion.
            Some(_) => {
                while let Ok(wait) = waits.try_recv() {
                    self.record_input(&context, &mut output, wait).await?;
                }
            }
            None => toolbox.cancel(&context).await,
        }
        Ok(outcome)
    }

    async fn record_output(
        &self,
        context: &CallContext,
        output: &mut watch::Receiver<Option<(String, u64)>>,
    ) -> Result<(), ConversationError> {
        if let Some(event) = self.output_event(context, output) {
            self.record(vec![event]).await?;
        }
        Ok(())
    }

    /// Records a change of the call's input wait, after the output that came before it
    /// when that was not recorded yet, so a client sees the prompt before it asks.
    async fn record_input(
        &self,
        context: &CallContext,
        output: &mut watch::Receiver<Option<(String, u64)>>,
        (input, looks_secret): (InputWait, bool),
    ) -> Result<(), ConversationError> {
        let mut events = Vec::with_capacity(2);
        if output.has_changed().unwrap_or(false) {
            events.extend(self.output_event(context, output));
        }
        events.push(Event::ToolCallInputChanged {
            turn_id: self.turn_id(),
            call_id: context.call_id,
            input,
            looks_secret,
        });
        self.record(events).await?;
        Ok(())
    }

    /// The newest output as an event, marked as seen.
    fn output_event(
        &self,
        context: &CallContext,
        output: &mut watch::Receiver<Option<(String, u64)>>,
    ) -> Option<Event> {
        let (tail, bytes) = output.borrow_and_update().clone()?;
        Some(Event::ToolCallOutputUpdated {
            turn_id: self.turn_id(),
            call_id: context.call_id,
            tail,
            bytes,
        })
    }

    /// Appends `events` of this conversation in one batch.
    async fn record(&self, events: Vec<Event>) -> Result<Committed, ConversationError> {
        let conversation_id = self.shared.conversation_id;
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(conversation_id, event));
        let committed =
            self.shared.deps.writer.append(batch).await.map_err(ConversationError::from_store)?;
        self.drafter.recorded(committed.last_seq());
        Ok(committed)
    }
}

/// What a parked approval ended with.
enum Waited {
    Answer(Option<ApprovalDecision>),
    Interrupted,
    TimedOut,
}

/// A tool call from the model's answer, before it has a daemon call id.
#[derive(Debug, Clone, PartialEq)]
struct PendingCall {
    /// The provider's id, which ties the result to the call in the next request.
    provider_call_id: String,
    name: String,
    input: Value,
    /// True when the model wrote the input as text for a freeform tool.
    freeform: bool,
}

/// Why the model stopped, for `turn_completed`, when the final answer `message`, which
/// stopped for `stop`, has no text and no tool call, or stopped for another reason than
/// the end of its answer, such as the output limit or a refusal. `None` for a normal
/// answer. A `pause_turn` ends the turn too: efr does not send the answer back to
/// continue it.
fn turn_stop(stop: Option<StopReason>, message: &Message) -> Option<TurnStop> {
    let empty = message.text().trim().is_empty() && tool_calls(message).is_empty();
    let reason = match stop {
        Some(StopReason::MaxTokens) => WireStop::MaxTokens,
        Some(StopReason::Refusal) => WireStop::Refusal,
        Some(StopReason::ContentFilter) => WireStop::ContentFilter,
        Some(StopReason::PauseTurn) => WireStop::PauseTurn,
        _ => WireStop::EndTurn,
    };
    (empty || reason != WireStop::EndTurn).then_some(TurnStop { reason, empty })
}

/// The tool calls of an assistant message, in order.
fn tool_calls(message: &Message) -> Vec<PendingCall> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { call_id, name, input, freeform } => Some(PendingCall {
                provider_call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
                freeform: *freeform,
            }),
            _ => None,
        })
        .collect()
}

/// The `home_file_changed` event of a file outside the roots that the call `call_id`
/// changed. A copy in efr's phase A backup store keeps its path, and undo cannot bring
/// the file back from it. From phase B the copy is in efr's home store, which has no
/// path of its own (an empty `backup`): the file is covered unless the toolbox names
/// why efr could not copy it.
fn home_file_changed(turn_id: TurnId, call_id: CallId, backup: FileBackup) -> Event {
    let phase_a = !backup.backup.as_os_str().is_empty();
    Event::HomeFileChanged {
        turn_id,
        call_id,
        path: backup.path,
        floor: backup.floor,
        covered: !phase_a && backup.reason.is_none(),
        backup: phase_a.then_some(backup.backup),
        real: backup.real,
    }
}

/// What the model reads when the user says no to a call of `tool` that is no exit.
pub(crate) fn user_denied(tool: &str) -> String {
    format!(
        "The user denied the {tool} call; it did not run. Do not run it again in another \
         form. Tell the user what you wanted to do."
    )
}

/// The `provider_options` of a request of the conversation `conversation_id`: those of
/// the config, and the conversation id as the prompt cache key unless the config names
/// one. The key sends every request of the conversation, the summary requests too, to
/// the same prompt cache, as Codex does with its session id. The effort is not here: it
/// is the request's own `effort`.
pub(crate) fn provider_options(
    config: &ConversationConfig,
    conversation_id: ConversationId,
) -> serde_json::Map<String, Value> {
    let mut options = config.provider_options.clone();
    options.entry(PROMPT_CACHE_KEY).or_insert_with(|| Value::String(conversation_id.to_string()));
    options
}

/// The tool with which `model` changes files: the one that the provider's model list
/// names for it. A model that the list does not name gets the tool that every listed
/// model shares, as all models of one provider do today, else the provider's own
/// default, so a Claude model gets `edit` also while the list is empty.
pub(crate) fn edit_tool(provider: &dyn Provider, model: &str) -> EditTool {
    let models = provider.models();
    if let Some(info) = models.iter().find(|info| info.id == model) {
        return info.edit_tool;
    }
    match models.split_first() {
        Some((first, rest)) if rest.iter().all(|info| info.edit_tool == first.edit_tool) => {
            first.edit_tool
        }
        _ => provider.default_edit_tool(),
    }
}

/// The context of a model call: its input, cached tokens included, plus its output.
fn context_tokens(usage: &TokenUsage) -> u64 {
    usage.input_tokens.saturating_add(usage.output_tokens)
}

/// The `turn_failed` body for a provider error: a code a client can act on and the
/// error's one-sentence message.
pub(crate) fn provider_failure(error: &ProviderError) -> ErrorBody {
    let code = match error {
        // NOTE: a token source that cannot produce a token has no usable credentials,
        // whether none were saved, the saved ones are of the wrong kind or the refresh
        // grant was refused; `unauthorized` is what tells the client to send the user
        // to `efr login`.
        ProviderError::Unauthorized { .. }
        | ProviderError::NotLoggedIn
        | ProviderError::Token { .. } => ErrorCode::Unauthorized,
        ProviderError::RateLimited { .. } | ProviderError::Overloaded { .. } => ErrorCode::Busy,
        ProviderError::UnknownModel { .. } => ErrorCode::Invalid,
        _ => ErrorCode::Internal,
    };
    let body = ErrorBody::new(code, error.to_string());
    match error {
        ProviderError::RateLimited { retry_after: Some(delay) } => {
            let millis = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
            body.with_data(serde_json::json!({ "retry_after_ms": millis }))
        }
        // NOTE: the model a client can tell the user to replace; which ids the
        // subscription serves to efr is known only once a request is refused.
        ProviderError::UnknownModel { model } => {
            body.with_data(serde_json::json!({ "model": model }))
        }
        ProviderError::ContextOverflow { .. } => {
            body.with_data(serde_json::json!({ "cause": context::CONTEXT_OVERFLOW }))
        }
        _ => body,
    }
}

/// Hands a tool's output updates to the turn, where only the newest matters, because
/// each update carries the whole tail; and every change of its input wait, which all
/// matter.
struct WatchSink {
    sender: watch::Sender<Option<(String, u64)>>,
    inputs: mpsc::Sender<(InputWait, bool)>,
}

impl OutputSink for WatchSink {
    fn update(&mut self, tail: &str, bytes: u64) {
        self.sender.send_replace(Some((bounded_tail(tail), bytes)));
    }

    fn input_changed(&mut self, wait: InputWait, looks_secret: bool) {
        // NOTE: the turn records each change as it comes, so the queue is full only if
        // the turn stopped reading; a lost change then matters to no one, and the
        // call's completion ends any wait.
        if self.inputs.try_send((wait, looks_secret)).is_err() {
            tracing::warn!("a change of a tool call's input wait was dropped");
        }
    }
}

/// The last [`TAIL_MAX`] bytes of `tail`, cut on a character boundary.
fn bounded_tail(tail: &str) -> String {
    let mut start = tail.len().saturating_sub(TAIL_MAX);
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    tail[start..].to_owned()
}

#[cfg(test)]
mod tests;
