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
//! `turn_interrupted`.
//!
//! `authorize_tool_call` is the only place in efr where a tool call meets the
//! permission engine. Tools declare (`efr-tools`, through the daemon's [`Toolbox`]),
//! `efr-permissions` decides, and this file enforces; nothing reaches
//! [`Toolbox::invoke`] without passing it.

mod coalesce;
mod stream;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use efr_permissions::{ConversationPolicy, Decision, DecisionInput, Effect};
use efr_protocol::{
    ApprovalDecision, CallId, ConversationId, ErrorBody, ErrorCode, Event, Origin, Scope,
    ShellContext, TurnId,
};
use efr_provider::{ContentBlock, Message, ProviderError, Request, Role, TokenUsage};
use efr_stdx::id::uuid_v7;
use efr_store::{Batch, Committed};
use serde_json::Value;
use tokio::sync::watch;
use tracing::Instrument as _;

use self::coalesce::{Coalescer, sleep_or_pending};
use self::stream::Response;
use crate::approvals::{self, Approvals};
use crate::history::{CachedTurn, Snapshot, close_open_calls};
use crate::interrupt::Interrupt;
use crate::preamble::LiveState;
use crate::scratch::Scratch;
use crate::steer::Steering;
use crate::{
    CallContext, ConversationConfig, ConversationDeps, ConversationError, OutputSink, ToolCall,
    ToolOutcome, Toolbox,
};

/// The longest output tail in a `tool_call_output_updated` event, in bytes.
const TAIL_MAX: usize = 4096;

/// What the model reads for a call that did not run because the user interrupted the
/// turn.
const NOT_RUN: &str = "The user interrupted the turn before this call ran.";

/// What the model reads for a call that was running when the user interrupted the turn.
const STOPPED: &str = "The user interrupted the turn while this call ran; it was stopped.";

/// What the model reads for a call whose approval request expired unanswered.
const EXPIRED: &str = "The approval request expired before the user answered; the call did \
                       not run.";

/// What every turn of one conversation shares.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) conversation_id: ConversationId,
    pub(crate) config: ConversationConfig,
    pub(crate) deps: ConversationDeps,
    pub(crate) scratch: Mutex<Scratch>,
    pub(crate) approvals: Approvals,
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
}

impl fmt::Debug for TurnSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: last_command is deliberately missing; see the type's doc comment.
        f.debug_struct("TurnSpec")
            .field("turn_id", &self.turn_id)
            .field("text", &self.text)
            .field("origin", &self.origin)
            .field("context", &self.context)
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
}

/// Runs the turn `spec` to its terminal event.
pub(crate) async fn run(
    shared: Arc<Shared>,
    spec: TurnSpec,
    control: Control,
    cache: HashMap<TurnId, Arc<CachedTurn>>,
) -> TurnEnd {
    let span = tracing::info_span!(
        "turn",
        conversation_id = %shared.conversation_id,
        turn_id = %spec.turn_id,
    );
    async move {
        let mut turn = Turn::new(shared, spec, control);
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
    /// The engine allowed the call, or the user approved it.
    Allowed,
    /// The engine refused the call, or the user denied it; the model reads `message`.
    Denied { message: String },
    /// The call could not be judged, such as an unknown tool; the model reads `message`.
    Refused { message: String },
    /// The user interrupted the turn while the call waited for approval.
    Interrupted,
    /// The approval request expired unanswered.
    Expired,
}

/// One running turn.
struct Turn {
    shared: Arc<Shared>,
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
    usage: Option<TokenUsage>,
}

impl Turn {
    fn new(shared: Arc<Shared>, spec: TurnSpec, control: Control) -> Self {
        let cwd = shared.deps.home.path().to_path_buf();
        let scratch = shared.config.scratch_root.clone();
        Turn {
            shared,
            spec,
            control,
            cwd,
            scope: Scope::Machine,
            scratch,
            transcript: Vec::new(),
            assistant_index: 0,
            usage: None,
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
        let turn_id = self.turn_id();
        let snapshot =
            Snapshot::read(&shared.deps.readers, shared.conversation_id, shared.config.history)
                .await?;
        let summary = snapshot.summary.as_ref();
        self.cwd = match (&self.spec.context, summary.and_then(|summary| summary.cwd.as_ref())) {
            (Some(context), _) => context.pwd.clone(),
            (None, Some(cwd)) => cwd.clone(),
            (None, None) => shared.deps.home.path().to_path_buf(),
        };
        let derivation = shared.deps.scope.resolve(&self.cwd).await;
        self.scope = derivation.scope.clone();
        let mut started =
            vec![Event::TurnStarted { turn_id, cwd: self.cwd.clone(), scope: self.scope.clone() }];
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

        let preamble = self.live_state(derivation.repo, snapshot.agent_cwd()).render();
        let mut messages =
            snapshot.history(turn_id, cache, shared.deps.provider.id(), shared.config.history);
        messages.push(Message::new(
            Role::User,
            vec![
                ContentBlock::Text { text: preamble },
                ContentBlock::Text { text: self.spec.text.clone() },
            ],
        ));
        let tools = shared.deps.toolbox.definitions();

        for _ in 0..shared.config.max_model_calls {
            if self.control.interrupt.is_raised() {
                return Ok(Ending::Interrupted);
            }
            for text in self.control.steering.take() {
                self.push(&mut messages, Message::user(text));
            }
            let request = Request {
                model: shared.config.model.clone(),
                system: shared.config.system_prompt.clone().filter(|system| !system.is_empty()),
                messages: messages.clone(),
                tools: tools.clone(),
                max_output_tokens: shared.config.max_output_tokens,
                provider_options: shared.config.provider_options.clone(),
            };
            let message = match self.respond(request).await? {
                Response::Done(message) => message,
                Response::Failed(error) => return Ok(Ending::Failed(provider_failure(&error))),
                Response::Interrupted => return Ok(Ending::Interrupted),
            };
            let calls = tool_calls(&message);
            self.push(&mut messages, message);
            if calls.is_empty() {
                if self.control.steering.is_waiting() {
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
            }
            self.push(&mut messages, Message::new(Role::User, results));
            if interrupted {
                return Ok(Ending::Interrupted);
            }
        }
        let limit = shared.config.max_model_calls;
        Ok(Ending::Failed(ErrorBody::new(
            ErrorCode::Internal,
            format!("the turn stopped after {limit} model calls without a final answer"),
        )))
    }

    /// Records the terminal event and hands the turn's messages back. A turn that
    /// stopped between a tool call and its result (a store error, say) leaves a call
    /// without a result, which later turns could not send; it gets an error result.
    async fn finish(mut self, ending: Ending) -> TurnEnd {
        let turn_id = self.turn_id();
        let event = match ending {
            Ending::Completed => {
                Event::TurnCompleted { turn_id, usage: self.usage.map(efr_protocol::Usage::from) }
            }
            Ending::Failed(error) => Event::TurnFailed { turn_id, error },
            Ending::Interrupted => Event::TurnInterrupted { turn_id },
        };
        if let Err(error) = self.record(vec![event]).await {
            tracing::error!(error = %error, "the end of the turn could not be recorded");
        }
        let provider = self.shared.deps.provider.id().clone();
        close_open_calls(&mut self.transcript);
        let cached = (!self.transcript.is_empty())
            .then_some(CachedTurn { provider, messages: self.transcript });
        TurnEnd { turn_id, cached }
    }

    /// Adds `message` to the request and to the turn's transcript.
    fn push(&mut self, messages: &mut Vec<Message>, message: Message) {
        messages.push(message.clone());
        self.transcript.push(message);
    }

    fn live_state(&self, repo: Option<efr_scope::Repo>, agent_cwd: Option<PathBuf>) -> LiveState {
        let context = self.spec.context.as_ref();
        let host = &self.shared.config.host;
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
        }
    }

    /// The conversation's `$SCRATCH`, named after the day it began and its title.
    async fn ensure_scratch(&self, snapshot: &Snapshot) -> Result<PathBuf, ConversationError> {
        let summary = snapshot.summary.as_ref();
        let began = summary.map_or_else(|| self.shared.deps.clock.now(), |s| s.created_at);
        let began = began.to_zoned(self.shared.config.time_zone.clone()).date();
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
            self.record(vec![Event::ToolCallStarted {
                turn_id,
                call_id,
                tool: call.name.clone(),
                input: call.input.clone(),
            }])
            .await?;
            let context = CallContext {
                conversation_id: self.shared.conversation_id,
                turn_id,
                call_id,
                cwd: self.cwd.clone(),
                scratch: self.scratch.clone(),
                scope: self.scope.clone(),
                origin: self.spec.origin,
            };
            let tool_call = ToolCall::new(call.name, call.input, context);
            let (outcome, interrupted) = if skip || self.control.interrupt.is_raised() {
                (ToolOutcome::error(NOT_RUN), true)
            } else {
                match self.authorize_tool_call(&tool_call).await? {
                    Authorization::Allowed => match self.invoke(tool_call).await? {
                        Some(outcome) => (outcome, false),
                        None => (ToolOutcome::error(STOPPED), true),
                    },
                    Authorization::Denied { message } | Authorization::Refused { message } => {
                        (ToolOutcome::error(message), false)
                    }
                    Authorization::Interrupted => (ToolOutcome::error(NOT_RUN), true),
                    Authorization::Expired => (ToolOutcome::error(EXPIRED), false),
                }
            };
            self.record(vec![Event::ToolCallCompleted {
                turn_id,
                call_id,
                output: outcome.output.clone(),
                truncated: outcome.truncated,
                is_error: outcome.is_error,
                exit_code: outcome.exit_code,
            }])
            .await?;
            let result = ContentBlock::ToolResult {
                call_id: call.provider_call_id,
                output: outcome.output,
                is_error: outcome.is_error,
            };
            Ok((result, interrupted))
        }
        .instrument(span)
        .await
    }

    /// The single permission check point: may `call` run?
    ///
    /// The toolbox declares what the call needs, [`efr_permissions::Engine::decide`]
    /// judges it with the turn's scope, origin and the conversation's policy, and the
    /// effect is enforced here: `Allow` lets the call run, `Deny` gives the model an
    /// error that names each refused path with its class, and `Ask` records
    /// `approval_requested` and parks the turn until the user answers, the turn is
    /// interrupted, or the request expires.
    async fn authorize_tool_call(
        &mut self,
        call: &ToolCall,
    ) -> Result<Authorization, ConversationError> {
        let requirements = match self.shared.deps.toolbox.requirements(call) {
            Ok(requirements) => requirements,
            Err(message) => return Ok(Authorization::Refused { message }),
        };
        let input = DecisionInput {
            requirements,
            scope: call.context.scope.clone(),
            origin: call.context.origin,
            conversation_policy: ConversationPolicy::new(&call.context.scratch)
                .with_rules(self.shared.config.policy.clone()),
        };
        // NOTE: the engine is cloned out so the watch's read lock is not held while
        // the turn records events or waits for an answer.
        let engine = Arc::clone(&self.shared.deps.engine.borrow());
        let decision = engine.decide(&input);
        tracing::debug!(effect = %decision.effect(), "the permission engine decided");
        match decision.effect() {
            Effect::Allow => Ok(Authorization::Allowed),
            Effect::Deny => {
                Ok(Authorization::Denied { message: approvals::denial(&call.name, &decision) })
            }
            Effect::Ask => self.ask(call, &decision).await,
        }
    }

    /// Parks `call` until the user answers, the turn is interrupted, or the approval
    /// timeout passes.
    async fn ask(
        &mut self,
        call: &ToolCall,
        decision: &Decision,
    ) -> Result<Authorization, ConversationError> {
        let turn_id = self.turn_id();
        let call_id = call.context.call_id;
        let summary = approvals::summary(&call.name, decision);
        let diff_preview = self.shared.deps.toolbox.preview(call).await;
        let answer = self.shared.approvals.park(turn_id, call_id);
        let requested = Event::ApprovalRequested { turn_id, call_id, summary, diff_preview };
        if let Err(error) = self.record(vec![requested]).await {
            self.shared.approvals.withdraw(call_id);
            return Err(error);
        }
        let interrupt = self.control.interrupt.clone();
        let clock = Arc::clone(&self.shared.deps.clock);
        let timeout = self.shared.config.approval_timeout;
        let waited = tokio::select! {
            biased;
            () = interrupt.raised() => Waited::Interrupted,
            answer = answer => Waited::Answer(answer.ok()),
            () = sleep_or_pending(&*clock, timeout) => Waited::TimedOut,
        };
        match waited {
            Waited::Answer(Some(ApprovalDecision::Allow)) => Ok(Authorization::Allowed),
            // NOTE: a decision added to the protocol later denies, so a newer client
            // can never run a call that this build would not.
            Waited::Answer(Some(_)) => Ok(Authorization::Denied {
                message: format!("The user denied the {} call; it did not run.", call.name),
            }),
            Waited::Answer(None) => Ok(Authorization::Expired),
            Waited::Interrupted => {
                self.expire(call_id).await?;
                Ok(Authorization::Interrupted)
            }
            Waited::TimedOut => {
                self.expire(call_id).await?;
                Ok(Authorization::Expired)
            }
        }
    }

    /// Records that the approval of `call_id` can no longer be answered, unless an
    /// answer took it first.
    async fn expire(&self, call_id: CallId) -> Result<(), ConversationError> {
        if self.shared.approvals.withdraw(call_id) {
            self.record(vec![Event::ApprovalExpired { turn_id: self.turn_id(), call_id }]).await?;
        }
        Ok(())
    }

    /// Runs an authorized call, recording its output as it grows. `None` when the user
    /// interrupted it: the call's future is dropped and the toolbox asked to stop it.
    async fn invoke(&mut self, call: ToolCall) -> Result<Option<ToolOutcome>, ConversationError> {
        let toolbox: Arc<dyn Toolbox> = Arc::clone(&self.shared.deps.toolbox);
        let clock = Arc::clone(&self.shared.deps.clock);
        let interrupt = self.control.interrupt.clone();
        let context = call.context.clone();
        let (sender, mut output) = watch::channel(None);
        let mut sink = WatchSink { sender };
        let mut updates = Coalescer::new(self.shared.config.update_interval);
        let mut invoked = Box::pin(toolbox.invoke(call, &mut sink));
        let outcome = loop {
            let flush = updates.flush_after(clock.now());
            tokio::select! {
                biased;
                () = interrupt.raised() => break None,
                outcome = &mut invoked => break Some(outcome),
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
        if outcome.is_none() {
            toolbox.cancel(&context).await;
        }
        Ok(outcome)
    }

    async fn record_output(
        &self,
        context: &CallContext,
        output: &mut watch::Receiver<Option<(String, u64)>>,
    ) -> Result<(), ConversationError> {
        let latest = output.borrow_and_update().clone();
        if let Some((tail, bytes)) = latest {
            self.record(vec![Event::ToolCallOutputUpdated {
                turn_id: self.turn_id(),
                call_id: context.call_id,
                tail,
                bytes,
            }])
            .await?;
        }
        Ok(())
    }

    /// Appends `events` of this conversation in one batch.
    async fn record(&self, events: Vec<Event>) -> Result<Committed, ConversationError> {
        let conversation_id = self.shared.conversation_id;
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(conversation_id, event));
        self.shared.deps.writer.append(batch).await.map_err(ConversationError::from_store)
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
}

/// The tool calls of an assistant message, in order.
fn tool_calls(message: &Message) -> Vec<PendingCall> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall { call_id, name, input } => Some(PendingCall {
                provider_call_id: call_id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The `turn_failed` body for a provider error: a code a client can act on and the
/// error's one-sentence message.
pub(crate) fn provider_failure(error: &ProviderError) -> ErrorBody {
    let code = match error {
        ProviderError::Unauthorized | ProviderError::NotLoggedIn => ErrorCode::Unauthorized,
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
        _ => body,
    }
}

/// Hands a tool's output updates to the turn; only the newest matters, because each
/// update carries the whole tail.
struct WatchSink {
    sender: watch::Sender<Option<(String, u64)>>,
}

impl OutputSink for WatchSink {
    fn update(&mut self, tail: &str, bytes: u64) {
        self.sender.send_replace(Some((bounded_tail(tail), bytes)));
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
