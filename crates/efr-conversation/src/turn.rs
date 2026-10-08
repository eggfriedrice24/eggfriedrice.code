//! One turn, from a prompt to its terminal event, and the single permission check
//! point.
//!
//! A turn reads a snapshot of the conversation, derives the scope from the user's
//! working directory, records `turn_started` (and `scope_changed` when the user moved),
//! makes sure `$SCRATCH` exists, and assembles the request: the system prompt, bounded
//! history, the live-state preamble with the prompt, and the tool definitions. It then
//! streams the provider and records what comes back. Each tool call the model asks for
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
//! [`Launch::Contained`]. An exit asks the user, with its record in `exit_requested`
//! first, and a "yes" runs the call with the narrowest launch ([`exit::grant`]). An
//! exit that would run outside the sandbox must be one command, or the call is refused
//! with no question. A floor refuses an exit before any question, and three refusals in
//! a row without a person stop the turn. A call that changed git settings that run
//! programs asks the user whether to keep them before the next call.

mod coalesce;
mod compact;
mod drafter;
mod stream;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use efr_permissions::{ConversationPolicy, Decision, DecisionInput, Effect, Engine, Requirements};
use efr_protocol::{
    ApprovalDecision, CallId, CompactionTrigger, ConversationId, EffectiveSettings, ErrorBody,
    ErrorCode, Event, InputWait, JudgeKind, Launch, Mode, Origin, QuestionId, Scope, ShellContext,
    SurfaceChange, TurnId, TurnSettings, Verdict,
};
use efr_provider::{ContentBlock, Message, ProviderError, Request, Role, TokenUsage};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Stopwatch;
use efr_store::turn_messages::NewTurnMessages;
use efr_store::{Batch, Committed};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tracing::Instrument as _;

pub(crate) use self::compact::{read_fresh, summarized, wire_usage};

use self::coalesce::{Coalescer, sleep_or_pending};
use self::compact::{Compacted, Guard};
use self::drafter::Drafter;
use self::stream::Response;
use crate::approvals::{self, Approvals};
use crate::compaction::{Placed, Window, with_window};
use crate::exit::{self, TurnExits};
use crate::fresh::Fresh;
use crate::history::{CachedTurn, ModelKey, Snapshot, close_open_calls};
use crate::interrupt::Interrupt;
use crate::preamble::LiveState;
use crate::questions::Questions;
use crate::scratch::Scratch;
use crate::settings::{self, Place};
use crate::steer::Steering;
use crate::{
    CallContext, ConfigSource, ConversationConfig, ConversationDeps, ConversationError, OutputSink,
    ToolCall, ToolOutcome, Toolbox,
};

/// The longest output tail in a `tool_call_output_updated` event, in bytes.
const TAIL_MAX: usize = 4096;

/// How many changes of a call's input wait may wait for the turn to record them. A
/// command changes it at most once a second, and the turn records each at once.
const INPUT_CAPACITY: usize = 64;

/// The `provider_options` key of the reasoning effort, which the OpenAI provider reads.
const REASONING_EFFORT: &str = "reasoning_effort";

/// What the model reads for a call that did not run because the user interrupted the
/// turn.
const NOT_RUN: &str = "The user interrupted the turn before this call ran.";

/// What the model reads for a call that was running when the user interrupted the turn.
pub(crate) const STOPPED: &str =
    "The user interrupted the turn while this call ran; it was stopped.";

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
}

/// A prompt that waits for its turn, or runs as one.
///
/// `Debug` leaves out the last command, which can hold a secret.
#[derive(Clone)]
pub(crate) struct TurnSpec {
    pub(crate) turn_id: TurnId,
    pub(crate) text: String,
    pub(crate) origin: Origin,
    pub(crate) context: Option<ShellContext>,
    /// The last command of `prompt.send`. It lives only in memory, for the preamble.
    pub(crate) last_command: Option<String>,
    /// The settings the prompt asked for, resolved when the turn starts.
    pub(crate) settings: TurnSettings,
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
    /// The approval request expired unanswered.
    Expired,
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
    spec: TurnSpec,
    control: Control,
    cwd: PathBuf,
    scope: Scope,
    scratch: PathBuf,
    /// The turn's messages for the cache: the prompt without the preamble, then every
    /// message after it.
    transcript: Vec<Message>,
    /// The position of the next assistant message with text.
    assistant_index: u32,
    /// How many bytes of that message's text `assistant_message_updated` events hold.
    streamed: usize,
    /// The drafts of the turn, for live clients only.
    drafter: Drafter,
    usage: Option<TokenUsage>,
    /// The usage of the newest model call, until the estimate takes it as its base.
    last_call: Option<TokenUsage>,
    /// The base of the estimate: the real context of the newest call that reported its
    /// usage, and how many messages of the window it covered. `None` before the first
    /// such call and after a compaction.
    counted: Option<(u64, usize)>,
    /// The compactions in a row that left the context at or above the trigger, or freed
    /// nothing; at [`BREAKER_TRIES`](crate::BREAKER_TRIES) the turn stops compacting.
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
            shared.conversation_id,
            spec.turn_id,
            config.draft_interval,
        );
        Turn {
            shared,
            config,
            settings: None,
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
            counted: None,
            misses: 0,
            fresh,
            logged_shell_cwd: None,
            user_messages: Vec::new(),
            exits: TurnExits::default(),
            stop: None,
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
        let derivation = shared.deps.scope.resolve(&self.cwd).await;
        self.scope = derivation.scope.clone();
        // NOTE: resolved again against the settings of this moment, because the config
        // may have changed while the prompt waited; what no longer fits fails the turn.
        // `auto` needs the sandbox and a project below the home directory.
        let engine = Arc::clone(&shared.deps.engine.borrow());
        let sandbox = shared.deps.sandbox.borrow().clone();
        let project_root = match &self.scope {
            Scope::Project(id) => engine.locations().project_root(id),
            _ => None,
        };
        let place = Place { sandbox: &sandbox, project_root, home: engine.locations().home() };
        let settings =
            match settings::resolve(&self.spec.settings, &config, self.spec.origin, place) {
                Ok(settings) => settings,
                Err(error) => return Ok(Ending::Failed(settings::failure(&error))),
            };
        self.settings = Some(settings.clone());
        self.user_messages = exit::user_messages(&snapshot.page, turn_id);
        let mut started = vec![Event::TurnStarted {
            turn_id,
            cwd: self.cwd.clone(),
            scope: self.scope.clone(),
            settings: Some(settings.clone()),
        }];
        if let Some(previous) = summary.and_then(|summary| summary.scope.clone())
            && previous != self.scope
        {
            started.push(Event::ScopeChanged { turn_id, from: previous, to: self.scope.clone() });
        }
        self.record(started).await?;
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
            Some(compaction) => Some(self.fresh_for(compaction.compaction_id).await),
            None => None,
        };
        let mut window = snapshot.window(
            Some(turn_id),
            cache,
            &self.model_key(),
            config.history,
            fresh.as_deref(),
        );
        window.placed.push(Placed {
            turn: turn_id,
            index: 0,
            message: Message::new(
                Role::User,
                vec![
                    ContentBlock::Text { text: preamble },
                    ContentBlock::Text { text: self.spec.text.clone() },
                ],
            ),
        });
        let mut provider_options = config.provider_options.clone();
        if let Some(effort) = &settings.effort {
            provider_options.insert(REASONING_EFFORT.to_owned(), Value::String(effort.clone()));
        }
        let base = Request {
            model: settings.model.clone(),
            system: config.system_prompt.clone().filter(|system| !system.is_empty()),
            messages: Vec::new(),
            tools: shared.deps.toolbox.definitions(),
            max_output_tokens: config.max_output_tokens,
            provider_options,
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
            let mut overflowed = false;
            let message = loop {
                match self.respond(with_window(&base, &window)).await? {
                    Response::Done(message) => break message,
                    // NOTE: never retried as a transient error: the same request fails
                    // again. One compaction and one more try; a second refusal ends the
                    // turn.
                    Response::Failed(error) if error.is_context_overflow() => {
                        if overflowed || !self.may_compact() {
                            return Ok(Ending::Failed(self.full(estimate)));
                        }
                        overflowed = true;
                        self.catch_up(&mut window);
                        let trigger = CompactionTrigger::Overflow;
                        match self.compact(trigger, estimate, &mut window, &base).await? {
                            Compacted::Done { .. } => {}
                            Compacted::NotDone => return Ok(Ending::Failed(self.full(estimate))),
                            Compacted::Interrupted => return Ok(Ending::Interrupted),
                        }
                    }
                    Response::Failed(error) => return Ok(Ending::Failed(provider_failure(&error))),
                    Response::Interrupted => return Ok(Ending::Interrupted),
                }
            };
            let calls = tool_calls(&message);
            self.push(&mut window, message);
            self.count(&window);
            if calls.is_empty() {
                if !self.control.steering.close_if_idle().await {
                    continue;
                }
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
        // NOTE: asked for every ending, so the toolbox keeps the last snapshot of an
        // interrupted or failed turn too; only a completed turn reports the changes.
        let watch = Stopwatch::start();
        let changes = self.shared.deps.toolbox.turn_changes(conversation_id, turn_id).await;
        tracing::debug!(phase = "turn_changes", elapsed_ms = %watch, "phase=turn_changes elapsed_ms={}", watch);
        let usage = self.usage.map(efr_protocol::Usage::from);
        let event = match ending {
            Ending::Completed => Event::TurnCompleted { turn_id, usage, context: None, changes },
            Ending::Failed(error) => Event::TurnFailed { turn_id, error, usage, context: None },
            Ending::Interrupted => Event::TurnInterrupted { turn_id, usage, context: None },
        };
        let key = self.model_key();
        close_open_calls(&mut self.transcript);
        let mut batch = Batch::new();
        // NOTE: the report comes before the terminal event, so a view that stops at
        // the end of the turn has shown it.
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
        TurnEnd { turn_id, cached, record: batch, fresh: self.fresh }
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
            self.config.history.max_turns,
        ))
    }

    /// The provider and the model that answer this turn.
    fn model_key(&self) -> ModelKey {
        let model = self.settings.as_ref().map_or(&self.config.model, |settings| &settings.model);
        ModelKey::new(self.shared.deps.provider.id().clone(), model.clone())
    }

    /// The turn's permission mode; the config's until the turn has resolved its own.
    fn mode(&self) -> Mode {
        self.settings.as_ref().map_or(self.config.mode, |settings| settings.mode)
    }

    /// Adds `message` to the request's window and to the turn's transcript, where its
    /// place is.
    fn push(&mut self, window: &mut Window, message: Message) {
        let index = u32::try_from(self.transcript.len()).unwrap_or(u32::MAX);
        window.placed.push(Placed { turn: self.turn_id(), index, message: message.clone() });
        self.transcript.push(message);
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
                auto: efr_permissions::effective_mode(self.mode(), self.spec.origin) == Mode::Auto,
            };
            let mut tool_call = ToolCall::new(call.name, call.input, context);
            let skipped = skip || self.control.interrupt.is_raised();
            // NOTE: judged before the start is recorded, so `tool_call_started` says how
            // the call runs; the engine is pure, and nothing runs until it allows.
            let judged = if skipped { None } else { Some(self.judge(&tool_call).await) };
            let launch = match &judged {
                Some(Judged::Ruled(ruling)) => ruling.planned_launch(),
                _ => None,
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
                None => (ToolOutcome::error(NOT_RUN), true),
                Some(judged) => match self.authorize_tool_call(&tool_call, judged).await? {
                    (Authorization::Allowed { approved_interactive, launch }, exits) => {
                        tool_call.context.approved_interactive = approved_interactive;
                        tool_call.context.launch = launch;
                        tool_call.context.exits = exits;
                        let watch = Stopwatch::start();
                        let invoked = self.invoke(tool_call.clone()).await?;
                        tracing::debug!(phase = "tool_run", elapsed_ms = %watch, "phase=tool_run elapsed_ms={}", watch);
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
                    (Authorization::Expired, _) => (ToolOutcome::error(EXPIRED), false),
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
        tracing::debug!(phase = "exit_prediction", elapsed_ms = %watch, "phase=exit_prediction elapsed_ms={}", watch);
        Judged::Ruled(Box::new(Ruling { requirements, decision, launch, problem, engine }))
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
            Effect::Allow => Authorization::Allowed {
                approved_interactive: false,
                launch: ruling.launch.clone(),
            },
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
            },
        ];
        self.record(events).await?;
        self.exits.judged(&floors, Verdict::Deny);
        if self.exits.refused() {
            self.stop = Some(ErrorBody::new(ErrorCode::Forbidden, exit::REFUSALS_STOPPED));
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
        let summary = approvals::summary(&call.name, decision);
        let diff_preview = self.shared.deps.toolbox.preview(call).await;
        let kinds = exit::kinds(decision);
        let mut events = Vec::with_capacity(2);
        let exit = if kinds.is_empty() {
            None
        } else {
            let home = ruling.engine.locations().home();
            let info = exit::info(decision, &ruling.requirements, &ruling.launch, home);
            events.push(Event::ExitRequested {
                turn_id,
                call_id,
                kinds: kinds.clone(),
                grants: ruling.launch.grants().to_vec(),
                source: exit::source(decision),
                record: Box::new(self.exit_record(call, ruling)),
            });
            Some(info)
        };
        let answer = self.shared.approvals.park(turn_id, call_id);
        events.push(Event::ApprovalRequested {
            turn_id,
            call_id,
            summary,
            diff_preview,
            interactive,
            exit,
        });
        if let Err(error) = self.record(events).await {
            self.shared.approvals.withdraw(call_id);
            return Err(error);
        }
        tracing::debug!(phase = "approval_request", elapsed_ms = %watch, "phase=approval_request elapsed_ms={}", watch);
        let interrupt = self.control.interrupt.clone();
        let clock = Arc::clone(&self.shared.deps.clock);
        // NOTE: read at each call, not at turn start, so a reload reaches the next
        // approval of a running turn.
        let timeout = self.shared.config.current().approval_timeout;
        let watch = Stopwatch::start();
        let waited = tokio::select! {
            biased;
            () = interrupt.raised() => Waited::Interrupted,
            answer = answer => Waited::Answer(answer.ok()),
            () = sleep_or_pending(&*clock, timeout) => Waited::TimedOut,
        };
        tracing::debug!(phase = "approval_wait", elapsed_ms = %watch, "phase=approval_wait elapsed_ms={}", watch);
        let authorization = match waited {
            Waited::Answer(Some(ApprovalDecision::Allow)) => Authorization::Allowed {
                approved_interactive: interactive,
                launch: ruling.launch.clone(),
            },
            // NOTE: a decision added to the protocol later denies, so a newer client
            // can never run a call that this build would not.
            Waited::Answer(Some(_)) if !kinds.is_empty() => {
                Authorization::Denied { message: exit::EXIT_DENIED.to_owned(), refusal: None }
            }
            Waited::Answer(Some(_)) => Authorization::Denied {
                message: format!("The user denied the {} call; it did not run.", call.name),
                refusal: None,
            },
            Waited::Answer(None) => return Ok(Authorization::Expired),
            Waited::Interrupted => {
                self.expire(call_id).await?;
                return Ok(Authorization::Interrupted);
            }
            Waited::TimedOut => {
                self.expire(call_id).await?;
                return Ok(Authorization::Expired);
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
            }])
            .await?;
        }
        Ok(authorization)
    }

    /// Records that the approval of `call_id` can no longer be answered, unless an
    /// answer took it first.
    async fn expire(&self, call_id: CallId) -> Result<(), ConversationError> {
        if self.shared.approvals.withdraw(call_id) {
            self.record(vec![Event::ApprovalExpired { turn_id: self.turn_id(), call_id }]).await?;
        }
        Ok(())
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

/// The `turn_failed` body for a provider error: a code a client can act on and the
/// error's one-sentence message.
pub(crate) fn provider_failure(error: &ProviderError) -> ErrorBody {
    let code = match error {
        // NOTE: a token source that cannot produce a token has no usable credentials,
        // whether none were saved, the saved ones are of the wrong kind or the refresh
        // grant was refused; `unauthorized` is what tells the client to send the user
        // to `efr login`.
        ProviderError::Unauthorized | ProviderError::NotLoggedIn | ProviderError::Token { .. } => {
            ErrorCode::Unauthorized
        }
        ProviderError::RateLimited { .. } => ErrorCode::Busy,
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
