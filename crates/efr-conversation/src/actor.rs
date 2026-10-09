//! One actor per conversation: the prompt queue and the turns.
//!
//! [`ConversationActor::spawn`] starts a task that owns the conversation's state and
//! returns its [`ConversationHandle`], the only way in. Requests arrive on a bounded
//! mailbox and are answered on a `oneshot`. A turn runs in a task of its own, so the
//! actor keeps answering (steering, interrupts, approvals, more prompts) while the
//! model streams. Prompts that arrive while a turn runs queue behind it, one turn at a
//! time.
//!
//! The actor, not the turn, records a turn's terminal event, and only after it has
//! cleared the running turn. The actor answers one request at a time, so a request that
//! a client sends after it sees the end finds no running turn: a steer or an interrupt
//! is refused, and a prompt starts at once.
//!
//! A steer is late when the running turn will make no more model calls: its steering
//! closed, an interrupt was asked for, it names another turn, or no turn runs. A late
//! steer is never recorded as `turn_steered`; with `if_late` it becomes a queued prompt
//! in the same step. Queued prompts can be withdrawn until they start. An interrupt can
//! withdraw queued prompts, take back unread steers and send unread steers again as a
//! new prompt, all in the step that records `turn_interrupt_requested`, so no queued
//! prompt starts and no model call reads a steer in between.
//!
//! A manual compaction (`conversation.compact`) runs only while no turn runs, in a task
//! of its own; a prompt that arrives meanwhile queues behind it, and the compaction
//! never starts a turn itself.
//!
//! Every request that changes the conversation records its event and the command's
//! receipt in one batch, so a retried command id returns the stored result and nothing
//! runs twice. A result that reports a sequence number is stored without it; the store
//! records the number with the receipt, and the daemon completes the result from it
//! ([`completed_result`]) when it answers a retry.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

mod compact;

use efr_protocol::{
    ApprovalRespond, ApprovalRespondResult, CallId, CommandId, ConversationCompact,
    ConversationCompactResult, ConversationId, EffectiveSettings, ErrorBody, ErrorCode, Event,
    LateSteer, Mode, Origin, PromptSend, PromptSendResult, PromptWithdraw, PromptWithdrawResult,
    QuestionId, ResentSteers, SandboxSurfaceRespond, SandboxSurfaceRespondResult, Seq,
    ShellContext, TurnId, TurnInterrupt, TurnInterruptResult, TurnSettings, TurnSteer,
    TurnSteerResult, WithdrawTarget, WithdrawnPrompt, WithdrawnSteer,
};
use efr_stdx::id::uuid_v7;
use efr_store::receipts::NewReceipt;
use efr_store::{Batch, Committed};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinHandle};
use tracing::Instrument as _;

use crate::approvals::Approvals;
use crate::drafts::{ConversationDraft, LiveStatus};
use crate::fresh::Fresh;
use crate::history::CachedTurn;
use crate::questions::Questions;
use crate::scratch::Scratch;
use crate::settings;
use crate::steer::Reservation;
use crate::turn::{self, Control, Shared, TurnEnd, TurnSpec};
use crate::{ConfigSource, ConversationDeps, ConversationError, ConversationStart};

/// Requests waiting for the actor; senders wait when it is full.
const MAILBOX: usize = 64;

const PROMPT_SEND: &str = "prompt.send";
const TURN_STEER: &str = "turn.steer";
const TURN_INTERRUPT: &str = "turn.interrupt";
const APPROVAL_RESPOND: &str = "approval.respond";
const SURFACE_RESPOND: &str = "sandbox.surface_respond";
const PROMPT_WITHDRAW: &str = "prompt.withdraw";

/// The actor of one conversation. It runs in its own task; the
/// [`ConversationHandle`] that [`spawn`](Self::spawn) returns is the only way to reach
/// it.
#[derive(Debug)]
pub struct ConversationActor {
    shared: Arc<Shared>,
    mailbox: mpsc::Receiver<Message>,
    /// `New` until the first prompt records `conversation_created`.
    start: ConversationStart,
    queue: VecDeque<TurnSpec>,
    running: Option<Running>,
    /// The exact messages of the turns this actor ran that the newest request carried,
    /// for their provider items.
    cache: HashMap<TurnId, Arc<CachedTurn>>,
    /// The fresh context block of a compaction from an efrd before the stored block,
    /// read from disk once, so every request until the next compaction sends the same
    /// bytes.
    fresh: Option<Fresh>,
    /// The manual compaction that runs, with the caller that waits for it.
    compacting: Option<Compacting>,
}

/// A manual compaction in its task, and the callers that wait for its answer.
#[derive(Debug)]
struct Compacting {
    task: JoinHandle<compact::Compacted>,
    reply: Reply<ConversationCompactResult>,
    /// The command that started it.
    command_id: CommandId,
    /// The retries of the same command that came while it ran.
    retries: Vec<Reply<ConversationCompactResult>>,
}

/// A cheap, cloneable handle to a conversation's actor.
#[derive(Debug, Clone)]
pub struct ConversationHandle {
    conversation_id: ConversationId,
    mailbox: mpsc::Sender<Message>,
    status: Arc<LiveStatus>,
}

/// What a conversation is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversationState {
    /// The running turn.
    pub running: Option<TurnId>,
    /// The turns waiting behind it, oldest first.
    pub queued: Vec<TurnId>,
    /// The tool calls that wait for the user's approval.
    pub pending_approvals: Vec<CallId>,
    /// The quarantine questions that wait for the user's answer.
    pub pending_questions: Vec<QuestionId>,
    /// True while a manual compaction runs.
    pub compacting: bool,
}

#[derive(Debug)]
struct Running {
    turn_id: TurnId,
    task: JoinHandle<TurnEnd>,
    control: Control,
    /// The prompt of the turn. Unread steers that an interrupt sends again take its
    /// context and settings.
    spec: TurnSpec,
    /// Set once `turn_interrupt_requested` is recorded: a steer after it is late.
    interrupt_requested: bool,
}

/// Where a recorded prompt joins the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueEnd {
    /// It runs next: the unread steers of an interrupted turn.
    Front,
    /// It waits behind the others.
    Back,
}

/// A prompt to record and queue: from `prompt.send`, from a late steer, or from the
/// unread steers of an interrupted turn.
///
/// `Debug` leaves out the last command, which can hold a secret.
struct NewPrompt {
    command_id: CommandId,
    text: String,
    origin: Origin,
    context: Option<ShellContext>,
    /// Only for the turn's preamble; never recorded.
    last_command: Option<String>,
    settings: TurnSettings,
    /// The steers that an interrupt sends again as this prompt.
    steers: Vec<Seq>,
}

impl std::fmt::Debug for NewPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // NOTE: last_command is deliberately missing; see the type's doc comment.
        f.debug_struct("NewPrompt")
            .field("command_id", &self.command_id)
            .field("text", &self.text)
            .field("origin", &self.origin)
            .field("context", &self.context)
            .field("settings", &self.settings)
            .field("steers", &self.steers)
            .finish_non_exhaustive()
    }
}

impl NewPrompt {
    /// The `prompt_queued` event of this prompt as the turn `turn_id`.
    fn event(&self, turn_id: TurnId) -> Event {
        Event::PromptQueued {
            turn_id,
            command_id: self.command_id,
            text: self.text.clone(),
            origin: self.origin,
            context: self.context.clone(),
            settings: self.settings.clone(),
            steers: self.steers.clone(),
        }
    }

    /// The prompt as the turn `turn_id` runs it.
    fn into_spec(self, turn_id: TurnId) -> TurnSpec {
        TurnSpec {
            turn_id,
            text: self.text,
            origin: self.origin,
            context: self.context,
            last_command: self.last_command,
            settings: self.settings,
        }
    }
}

#[derive(Debug)]
enum Message {
    // NOTE: boxed because a prompt's params are large and the mailbox holds 64 slots.
    Request(Box<Request>),
    Shutdown(oneshot::Sender<()>),
}

type Reply<T> = oneshot::Sender<Result<T, ConversationError>>;

#[derive(Debug)]
enum Request {
    // NOTE: boxed so the other requests stay small; a prompt's params are the largest.
    Prompt {
        params: Box<PromptSend>,
        origin: Origin,
        reply: Reply<PromptSendResult>,
    },
    Steer {
        params: TurnSteer,
        origin: Origin,
        reply: Reply<TurnSteerResult>,
    },
    Interrupt {
        params: TurnInterrupt,
        origin: Origin,
        reply: Reply<TurnInterruptResult>,
    },
    Withdraw {
        params: PromptWithdraw,
        origin: Origin,
        reply: Reply<PromptWithdrawResult>,
    },
    Approval {
        params: ApprovalRespond,
        origin: Origin,
        reply: Reply<ApprovalRespondResult>,
    },
    Surface {
        params: SandboxSurfaceRespond,
        origin: Origin,
        reply: Reply<SandboxSurfaceRespondResult>,
    },
    Compact {
        params: ConversationCompact,
        reply: Reply<ConversationCompactResult>,
    },
    State {
        reply: oneshot::Sender<ConversationState>,
    },
}

impl ConversationActor {
    /// Starts the actor of `conversation_id` in a new task and returns its handle.
    ///
    /// A [`ConversationStart::New`] conversation is recorded with its first prompt. The
    /// actor runs until [`ConversationHandle::shutdown`], or until every handle is
    /// dropped and the running and queued turns have finished. Must be called inside a
    /// tokio runtime.
    pub fn spawn(
        conversation_id: ConversationId,
        start: ConversationStart,
        config: Arc<dyn ConfigSource>,
        deps: ConversationDeps,
    ) -> ConversationHandle {
        let (sender, mailbox) = mpsc::channel(MAILBOX);
        let scratch = Mutex::new(Scratch::new(&config.current().scratch_root, conversation_id));
        let shared = Arc::new(Shared {
            conversation_id,
            config,
            deps,
            scratch,
            approvals: Approvals::default(),
            questions: Questions::default(),
            misses: AtomicU32::new(0),
            status: Arc::default(),
        });
        let status = Arc::clone(&shared.status);
        let actor = ConversationActor {
            shared,
            mailbox,
            start,
            queue: VecDeque::new(),
            running: None,
            cache: HashMap::new(),
            fresh: None,
            compacting: None,
        };
        let span = tracing::info_span!("conversation", conversation_id = %conversation_id);
        tokio::spawn(actor.run().instrument(span));
        ConversationHandle { conversation_id, mailbox: sender, status }
    }

    async fn run(mut self) {
        let mut open = true;
        let mut stopped_reply = None;
        while open || self.running.is_some() || self.compacting.is_some() {
            tokio::select! {
                message = self.mailbox.recv(), if open => match message {
                    Some(Message::Request(request)) => self.handle(*request).await,
                    Some(Message::Shutdown(reply)) => {
                        stopped_reply = Some(reply);
                        break;
                    }
                    None => open = false,
                },
                ended = turn_end(&mut self.running) => self.turn_ended(ended).await,
                ended = compaction_end(&mut self.compacting) => self.compaction_ended(ended),
            }
        }
        if let Some(compacting) = self.compacting.take() {
            // NOTE: nothing is recorded until the summary is back, so a compaction that
            // stops here leaves no trace; its caller gets `Stopped`.
            compacting.task.abort();
        }
        if let Some(running) = self.running.take() {
            // NOTE: the turn's events stop where it was; the reconciliation at the next
            // daemon start cancels the turn and expires its approvals.
            running.task.abort();
            // Waiting for the cancelled task means nothing of the turn runs once the
            // shutdown is answered. A turn that had already ended still gets its end.
            if let Ok(end) = running.task.await {
                self.record_end(end.record).await;
            }
        }
        if let Some(reply) = stopped_reply {
            let _ = reply.send(());
        }
    }

    async fn handle(&mut self, request: Request) {
        // A caller that stopped waiting does not need the answer; the work is done.
        match request {
            Request::Prompt { params, origin, reply } => {
                let _ = reply.send(self.prompt(*params, origin).await);
            }
            Request::Steer { params, origin, reply } => {
                let _ = reply.send(self.steer(params, origin).await);
            }
            Request::Interrupt { params, origin, reply } => {
                let _ = reply.send(self.interrupt(params, origin).await);
            }
            Request::Withdraw { params, origin, reply } => {
                let _ = reply.send(self.withdraw(params, origin).await);
            }
            Request::Approval { params, origin, reply } => {
                let _ = reply.send(self.respond(params, origin).await);
            }
            Request::Surface { params, origin, reply } => {
                let _ = reply.send(self.respond_surface(params, origin).await);
            }
            Request::Compact { params, reply } => self.compact(params, reply),
            Request::State { reply } => {
                let _ = reply.send(self.state());
            }
        }
    }

    fn conversation_id(&self) -> ConversationId {
        self.shared.conversation_id
    }

    async fn prompt(
        &mut self,
        params: PromptSend,
        origin: Origin,
    ) -> Result<PromptSendResult, ConversationError> {
        self.check_conversation(params.conversation_id)?;
        let effective = self.admit(&params.settings, origin)?;
        let turn_id = self.new_turn_id();
        let queued = self.running.is_some() || self.compacting.is_some() || !self.queue.is_empty();
        let prompt = NewPrompt {
            command_id: params.command_id,
            text: params.text,
            origin,
            context: params.context,
            last_command: params.last_command,
            settings: params.settings,
            steers: Vec::new(),
        };
        let (events, index) = self.queued_events(turn_id, &prompt);
        let result = PromptSendResult {
            conversation_id: self.conversation_id(),
            turn_id,
            seq: Seq::ZERO,
            queued,
            settings: Some(effective),
        };
        let receipt = receipt(prompt.command_id, PROMPT_SEND, &result)?.seq_of_event(index);
        let committed = self.append(events, receipt).await?;
        self.enqueue(turn_id, prompt, QueueEnd::Back);
        Ok(PromptSendResult { seq: seq_of(&committed, index), ..result })
    }

    /// Checks, before anything is recorded, that a prompt with `settings` from
    /// `origin` may queue: the queue has room, and the settings can work.
    fn admit(
        &self,
        settings: &TurnSettings,
        origin: Origin,
    ) -> Result<EffectiveSettings, ConversationError> {
        let config = self.shared.config.current();
        let limit = config.max_queued;
        if self.queue.len() >= limit {
            return Err(ConversationError::QueueFull {
                conversation_id: self.conversation_id(),
                limit,
            });
        }
        self.resolve(settings, origin)
    }

    /// The settings that a prompt with `settings` from `origin` runs with now, or the
    /// error that says why they cannot work.
    fn resolve(
        &self,
        settings: &TurnSettings,
        origin: Origin,
    ) -> Result<EffectiveSettings, ConversationError> {
        let config = self.shared.config.current();
        // NOTE: checked here so a value that cannot work fails before anything is
        // recorded; the turn checks again when it starts, against the settings of then,
        // when it also knows its project.
        let sandbox = self.shared.deps.sandbox.borrow().clone();
        let engine = Arc::clone(&self.shared.deps.engine.borrow());
        let place = settings::Place {
            sandbox: &sandbox,
            project_root: None,
            home: engine.locations().home(),
        };
        settings::resolve(settings, &config, origin, place)
    }

    fn new_turn_id(&self) -> TurnId {
        TurnId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng))
    }

    /// The events that record `prompt` as the turn `turn_id`, with
    /// `conversation_created` first for a conversation that is not recorded yet, and
    /// the index of `prompt_queued` among them.
    fn queued_events(&self, turn_id: TurnId, prompt: &NewPrompt) -> (Vec<Event>, usize) {
        let mut events = Vec::with_capacity(2);
        if let ConversationStart::New { origin, tty } = &self.start {
            events.push(Event::ConversationCreated { origin: *origin, tty: tty.clone() });
        }
        let index = events.len();
        events.push(prompt.event(turn_id));
        (events, index)
    }

    /// Takes a recorded prompt into the queue, and starts it when nothing runs.
    fn enqueue(&mut self, turn_id: TurnId, prompt: NewPrompt, end: QueueEnd) {
        self.start = ConversationStart::Existing;
        let spec = prompt.into_spec(turn_id);
        match end {
            QueueEnd::Front => self.queue.push_front(spec),
            QueueEnd::Back => self.queue.push_back(spec),
        }
        self.start_next();
    }

    /// Records a steer for the running turn, or, when the steer is late, refuses it or
    /// turns it into a queued prompt as `if_late` says. A late steer is never recorded
    /// as `turn_steered`.
    async fn steer(
        &mut self,
        params: TurnSteer,
        origin: Origin,
    ) -> Result<TurnSteerResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let late = match self.reserve_steer(params.turn_id) {
            Ok((turn_id, reservation)) => {
                let event = Event::TurnSteered { turn_id, text: params.text.clone() };
                let result = TurnSteerResult { turn_id, seq: Seq::ZERO, queued: false };
                let receipt = receipt(params.command_id, TURN_STEER, &result)?;
                let committed = self.append(vec![event], receipt).await?;
                let seq = committed.last_seq();
                reservation.push(seq, params.text);
                return Ok(TurnSteerResult { seq, ..result });
            }
            Err(late) => late,
        };
        let (context, last_command, settings) = match params.if_late {
            None => return Err(late),
            Some(LateSteer::Queue { context, last_command, settings }) => {
                (context, last_command, settings)
            }
            Some(_) => return Err(ConversationError::Unsupported { what: "if_late" }),
        };
        self.admit(&settings, origin)?;
        let turn_id = self.new_turn_id();
        let prompt = NewPrompt {
            command_id: params.command_id,
            text: params.text,
            origin,
            context,
            last_command,
            settings,
            steers: Vec::new(),
        };
        let (events, index) = self.queued_events(turn_id, &prompt);
        let result = TurnSteerResult { turn_id, seq: Seq::ZERO, queued: true };
        let receipt = receipt(prompt.command_id, TURN_STEER, &result)?.seq_of_event(index);
        let committed = self.append(events, receipt).await?;
        self.enqueue(turn_id, prompt, QueueEnd::Back);
        Ok(TurnSteerResult { seq: seq_of(&committed, index), ..result })
    }

    /// A place for a steer in the running turn, or the error that refuses a late
    /// steer: no turn runs, the request names another turn, an interrupt was asked
    /// for, or the turn made its last model call.
    fn reserve_steer(
        &self,
        requested: Option<TurnId>,
    ) -> Result<(TurnId, Reservation), ConversationError> {
        let turn_id = self.running_turn(requested)?;
        let no_turn =
            || ConversationError::NoRunningTurn { conversation_id: self.conversation_id() };
        let Some(running) = self.running.as_ref() else {
            return Err(no_turn());
        };
        // NOTE: a turn that is being interrupted, or that made its last model call,
        // reads no more steering; recording the steer would leave it never read.
        if running.interrupt_requested {
            return Err(no_turn());
        }
        let reservation = running.control.steering.reserve().ok_or_else(no_turn)?;
        Ok((turn_id, reservation))
    }

    /// Asks the running turn to stop. In the same step and the same append, it
    /// withdraws the listed queued prompts, takes back the listed unread steers that
    /// the client does not want sent, and sends the listed unread steers again as a new
    /// prompt that runs next, so no queued prompt starts and no model call reads a
    /// steer in between.
    async fn interrupt(
        &mut self,
        params: TurnInterrupt,
        origin: Origin,
    ) -> Result<TurnInterruptResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let turn_id = self.running_turn(params.turn_id)?;
        let Some(running) = self.running.as_ref() else {
            return Err(ConversationError::NoRunningTurn {
                conversation_id: self.conversation_id(),
            });
        };
        let resend_as = match params.resend_as.map(|resend_as| *resend_as) {
            None => None,
            Some(LateSteer::Queue { context, last_command, settings }) => {
                Some((context, last_command, settings))
            }
            Some(_) => return Err(ConversationError::Unsupported { what: "resend_as" }),
        };
        let withdrawn: Vec<(TurnId, String)> = self
            .queue
            .iter()
            .filter(|spec| params.withdraw.contains(&spec.turn_id))
            .map(|spec| (spec.turn_id, spec.text.clone()))
            .collect();
        // NOTE: taken out before the append, so the turn cannot read them while it
        // runs; they go back when the append fails. A steer listed in both lists is
        // sent again.
        let steers = running.control.steering.take_unread(&params.resend_steers);
        let kept = running.control.steering.take_unread(&params.withdraw_steers);
        let resend = (!steers.is_empty()).then(|| {
            let text = steers.iter().map(|steer| steer.text.as_str()).collect::<Vec<_>>();
            // NOTE: the interrupt never fails on account of the resend. The resent
            // prompt holds steers that the turn took already, so it does not count
            // against `max_queued`, and values of the terminal that cannot work now
            // give way to the turn's own, which did.
            let (context, last_command, settings) = match resend_as {
                Some((context, last_command, settings)) => {
                    let settings = match self.resolve(&settings, origin) {
                        Ok(_) => settings,
                        Err(error) => {
                            tracing::debug!(%error, "the steers go again with the settings of the interrupted turn");
                            running.spec.settings.clone()
                        }
                    };
                    (context, last_command, settings)
                }
                None => (
                    running.spec.context.clone(),
                    running.spec.last_command.clone(),
                    running.spec.settings.clone(),
                ),
            };
            let prompt = NewPrompt {
                command_id: params.command_id,
                text: text.join("\n"),
                origin,
                context,
                last_command,
                settings,
                steers: steers.iter().map(|steer| steer.seq).collect(),
            };
            (self.new_turn_id(), prompt)
        });
        let mut events = vec![Event::TurnInterruptRequested { turn_id, origin }];
        events.extend(
            withdrawn
                .iter()
                .map(|(turn_id, _)| Event::PromptWithdrawn { turn_id: *turn_id, origin }),
        );
        if !kept.is_empty() {
            let seqs = kept.iter().map(|steer| steer.seq).collect();
            events.push(Event::SteeringWithdrawn { turn_id, steers: seqs, origin });
        }
        events.extend(resend.as_ref().map(|(turn_id, prompt)| prompt.event(*turn_id)));
        let result = TurnInterruptResult {
            turn_id,
            seq: Seq::ZERO,
            resent: resend.as_ref().map(|(turn_id, prompt)| ResentSteers {
                turn_id: *turn_id,
                seq: Seq::ZERO,
                steers: prompt.steers.clone(),
            }),
            withdrawn: withdrawn
                .iter()
                .map(|(turn_id, text)| WithdrawnPrompt {
                    turn_id: *turn_id,
                    seq: Seq::ZERO,
                    text: text.clone(),
                })
                .collect(),
            withdrawn_steers: kept
                .iter()
                .map(|steer| WithdrawnSteer { seq: steer.seq, text: steer.text.clone() })
                .collect(),
        };
        let committed = match receipt(params.command_id, TURN_INTERRUPT, &result) {
            Ok(receipt) => self.append(events, receipt.seq_of_event(0)).await,
            Err(error) => Err(error),
        };
        let committed = match committed {
            Ok(committed) => committed,
            Err(error) => {
                if let Some(running) = &self.running {
                    running.control.steering.put_back(steers);
                    running.control.steering.put_back(kept);
                }
                return Err(error);
            }
        };
        if let Some(running) = &mut self.running {
            running.interrupt_requested = true;
            running.control.interrupt.raise();
        }
        self.queue.retain(|spec| !withdrawn.iter().any(|(turn_id, _)| *turn_id == spec.turn_id));
        let seq = seq_of(&committed, 0);
        let withdrawn = result
            .withdrawn
            .into_iter()
            .enumerate()
            .map(|(at, prompt)| WithdrawnPrompt { seq: seq_of(&committed, at + 1), ..prompt })
            .collect::<Vec<_>>();
        let resent_at = withdrawn.len() + 1 + usize::from(!kept.is_empty());
        let resent = result
            .resent
            .map(|resent| ResentSteers { seq: seq_of(&committed, resent_at), ..resent });
        if let Some((turn_id, prompt)) = resend {
            // NOTE: in front of the queue, so the steers run next, as the user typed
            // them before what waits.
            self.enqueue(turn_id, prompt, QueueEnd::Front);
        }
        Ok(TurnInterruptResult {
            turn_id,
            seq,
            resent,
            withdrawn,
            withdrawn_steers: result.withdrawn_steers,
        })
    }

    /// Takes one queued prompt out of the queue before it starts.
    async fn withdraw(
        &mut self,
        params: PromptWithdraw,
        origin: Origin,
    ) -> Result<PromptWithdrawResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let position = match &params.target {
            WithdrawTarget::Turn { turn_id } => {
                match self.queue.iter().position(|spec| spec.turn_id == *turn_id) {
                    Some(position) => position,
                    None => return Err(self.not_waiting(*turn_id).await),
                }
            }
            WithdrawTarget::NewestFromTty { tty } => self
                .queue
                .iter()
                .rposition(|spec| {
                    spec.context.as_ref().and_then(|context| context.tty.as_deref())
                        == Some(tty.as_str())
                })
                .ok_or_else(|| ConversationError::NoQueuedPrompt { tty: tty.clone() })?,
            _ => return Err(ConversationError::Unsupported { what: "withdraw target" }),
        };
        let (turn_id, text) = match self.queue.get(position) {
            Some(spec) => (spec.turn_id, spec.text.clone()),
            None => unreachable!("the position was found in the queue in this step"),
        };
        let event = Event::PromptWithdrawn { turn_id, origin };
        let result =
            PromptWithdrawResult { withdrawn: WithdrawnPrompt { turn_id, seq: Seq::ZERO, text } };
        let receipt = receipt(params.command_id, PROMPT_WITHDRAW, &result)?;
        let committed = self.append(vec![event], receipt).await?;
        // NOTE: the actor answers one request at a time and starts a queued turn only
        // between requests, so the prompt is still at `position`.
        self.queue.remove(position);
        let withdrawn = WithdrawnPrompt { seq: committed.last_seq(), ..result.withdrawn };
        Ok(PromptWithdrawResult { withdrawn })
    }

    /// Why the prompt of `turn_id` cannot be withdrawn: it no longer waits, or this
    /// conversation never queued it.
    async fn not_waiting(&self, turn_id: TurnId) -> ConversationError {
        let conversation_id = self.conversation_id();
        if self.running.as_ref().is_some_and(|running| running.turn_id == turn_id) {
            return ConversationError::PromptNotWaiting { turn_id };
        }
        let found = self
            .shared
            .deps
            .readers
            .with(move |conn| efr_store::conversations::turn(conn, turn_id))
            .await;
        match found {
            Ok(Some(turn)) if turn.conversation_id == conversation_id => {
                ConversationError::PromptNotWaiting { turn_id }
            }
            Ok(_) => ConversationError::UnknownTurn { conversation_id, turn_id },
            Err(source) => ConversationError::from_store(source),
        }
    }

    async fn respond(
        &mut self,
        params: ApprovalRespond,
        origin: Origin,
    ) -> Result<ApprovalRespondResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let call_id = params.call_id;
        let approvals = &self.shared.approvals;
        let Some(turn_id) = approvals.waiting_turn(call_id) else {
            return Err(ConversationError::ApprovalNotPending { call_id });
        };
        let event = Event::ApprovalResolved { turn_id, call_id, decision: params.decision, origin };
        let result = ApprovalRespondResult { seq: Seq::ZERO };
        let receipt = receipt(params.command_id, APPROVAL_RESPOND, &result)?;
        let committed = self.append(vec![event], receipt).await?;
        if !self.shared.approvals.answer(call_id, params.decision) {
            tracing::warn!(call_id = %call_id, "an approval was recorded after its turn stopped waiting");
        }
        Ok(ApprovalRespondResult { seq: committed.last_seq() })
    }

    /// Answers a quarantine question. The question is taken out before the answer is
    /// recorded, so a turn that expires at the same moment waits for this answer and
    /// records none of its own.
    async fn respond_surface(
        &mut self,
        params: SandboxSurfaceRespond,
        origin: Origin,
    ) -> Result<SandboxSurfaceRespondResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        // NOTE: a phone may not take a planted git setting out of quarantine; the
        // question stays open for the user's own terminal.
        if efr_permissions::effective_mode(Mode::Auto, origin) != Mode::Auto {
            return Err(ConversationError::RemoteSurfaceAnswer { origin });
        }
        let question_id = params.question_id;
        let Some(taken) = self.shared.questions.take(question_id) else {
            return Err(ConversationError::QuestionNotPending { question_id });
        };
        let event = Event::SurfaceQuestionAnswered {
            turn_id: taken.turn_id,
            question_id,
            keep: params.keep,
            origin: Some(origin),
        };
        let result = SandboxSurfaceRespondResult { seq: Seq::ZERO };
        // NOTE: when the answer cannot be recorded, `taken` drops unanswered and the
        // turn keeps the changes in quarantine.
        let receipt = receipt(params.command_id, SURFACE_RESPOND, &result)?;
        let committed = self.append(vec![event], receipt).await?;
        if !taken.answer(params.keep) {
            tracing::warn!(question_id = %question_id, "a quarantine answer was recorded after its turn stopped waiting");
        }
        Ok(SandboxSurfaceRespondResult { seq: committed.last_seq() })
    }

    /// Starts a manual compaction in a task of its own, or refuses it while a turn or
    /// another compaction runs. The answer goes to `reply` when the task ends.
    fn compact(&mut self, params: ConversationCompact, reply: Reply<ConversationCompactResult>) {
        if let Err(error) = self.check_conversation(Some(params.conversation_id)) {
            let _ = reply.send(Err(error));
            return;
        }
        // NOTE: a retry of the running command, as after a dropped connection, waits
        // for its answer: a refusal would be final and would throw the summary away.
        if let Some(compacting) =
            self.compacting.as_mut().filter(|compacting| compacting.command_id == params.command_id)
        {
            compacting.retries.push(reply);
            return;
        }
        // NOTE: the actor starts a queued prompt at once when nothing runs, so a queue
        // that is not empty means a turn or a compaction runs.
        if self.running.is_some() || self.compacting.is_some() {
            let conversation_id = self.conversation_id();
            let _ = reply.send(Err(ConversationError::CompactionBusy { conversation_id }));
            return;
        }
        let command_id = params.command_id;
        let job =
            compact::run(Arc::clone(&self.shared), params, self.cache.clone(), self.fresh.clone());
        let task = tokio::spawn(job.in_current_span());
        self.compacting = Some(Compacting { task, reply, command_id, retries: Vec::new() });
    }

    /// Answers the caller of the manual compaction that ended, keeps its fresh block,
    /// and starts the prompt that waited for it.
    fn compaction_ended(&mut self, ended: Result<compact::Compacted, JoinError>) {
        let Some(compacting) = self.compacting.take() else {
            return;
        };
        let answer = match ended {
            Ok(compacted) => {
                if let Some(fresh) = compacted.fresh {
                    self.fresh = Some(fresh);
                }
                if compacted.result.is_ok() {
                    // NOTE: the user made room; the next turn may compact on its own.
                    self.shared.misses.store(0, Ordering::Release);
                }
                compacted.result
            }
            Err(error) => {
                tracing::error!(error = %error, "the compaction's task stopped unexpectedly");
                Err(ConversationError::TaskPanicked { task: "compaction" })
            }
        };
        if let Err(error) = &answer {
            tracing::info!(error = %error, "the manual compaction did not compact");
        }
        for retry in compacting.retries {
            let _ = retry.send(match &answer {
                Ok(result) => Ok(result.clone()),
                Err(error) => Err(error.again()),
            });
        }
        let _ = compacting.reply.send(answer);
        self.start_next();
    }

    fn state(&self) -> ConversationState {
        ConversationState {
            running: self.running.as_ref().map(|running| running.turn_id),
            queued: self.queue.iter().map(|spec| spec.turn_id).collect(),
            pending_approvals: self.shared.approvals.parked(),
            pending_questions: self.shared.questions.parked(),
            compacting: self.compacting.is_some(),
        }
    }

    fn check_conversation(
        &self,
        requested: Option<ConversationId>,
    ) -> Result<(), ConversationError> {
        match requested {
            Some(requested) if requested != self.conversation_id() => {
                Err(ConversationError::WrongConversation {
                    conversation_id: self.conversation_id(),
                    requested,
                })
            }
            _ => Ok(()),
        }
    }

    /// The running turn, checked against the turn the request named.
    fn running_turn(&self, requested: Option<TurnId>) -> Result<TurnId, ConversationError> {
        let Some(running) = &self.running else {
            return Err(ConversationError::NoRunningTurn {
                conversation_id: self.conversation_id(),
            });
        };
        match requested {
            Some(requested) if requested != running.turn_id => {
                Err(ConversationError::TurnMismatch { running: running.turn_id, requested })
            }
            _ => Ok(running.turn_id),
        }
    }

    /// Starts the oldest queued turn when none runs.
    fn start_next(&mut self) {
        if self.running.is_some() || self.compacting.is_some() {
            return;
        }
        let Some(spec) = self.queue.pop_front() else {
            return;
        };
        let turn_id = spec.turn_id;
        let control = Control::new();
        let turn = turn::run(
            Arc::clone(&self.shared),
            spec.clone(),
            control.clone(),
            self.cache.clone(),
            self.fresh.clone(),
        );
        let task = tokio::spawn(turn.in_current_span());
        self.running = Some(Running { turn_id, task, control, spec, interrupt_requested: false });
    }

    /// Clears the running turn, then records its end. The order matters: a client
    /// that sees the end and at once steers or interrupts must find no running turn.
    async fn turn_ended(&mut self, ended: Result<TurnEnd, JoinError>) {
        let Some(running) = self.running.take() else {
            return;
        };
        if running.control.steering.is_waiting() {
            // NOTE: a turn that fails or is interrupted does not read the steering
            // that waits; it is rare enough to log rather than start a turn.
            tracing::warn!(turn_id = %running.turn_id, "steering waited when the turn ended");
        }
        // NOTE: before the end is recorded, so a client that attaches after the end
        // never gets the status of the ended turn.
        self.shared.status.clear(false);
        match ended {
            Ok(mut end) => {
                self.record_end(std::mem::take(&mut end.record)).await;
                if let Some(fresh) = end.fresh.take() {
                    self.fresh = Some(fresh);
                }
                self.remember(end);
            }
            Err(error) => self.turn_lost(running.turn_id, &error).await,
        }
        self.start_next();
    }

    /// Records the terminal batch of a turn that ended.
    async fn record_end(&self, record: Batch) {
        if let Err(error) = self.shared.deps.writer.append(record).await {
            tracing::error!(error = %error, "the end of the turn could not be recorded");
        }
    }

    /// Records the end of a turn whose task panicked, and expires its approvals.
    async fn turn_lost(&mut self, turn_id: TurnId, error: &JoinError) {
        tracing::error!(turn_id = %turn_id, error = %error, "the turn's task stopped unexpectedly");
        let mut events: Vec<Event> = self
            .shared
            .approvals
            .withdraw_turn(turn_id)
            .into_iter()
            .map(|call_id| Event::ApprovalExpired { turn_id, call_id })
            .collect();
        let error = ErrorBody::new(ErrorCode::Internal, "the turn stopped unexpectedly");
        events.push(Event::TurnFailed { turn_id, error, usage: None, context: None });
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(self.conversation_id(), event));
        if let Err(error) = self.shared.deps.writer.append(batch).await {
            tracing::error!(error = %error, "the end of a lost turn could not be recorded");
        }
    }

    /// Keeps the messages of a finished turn for the history of the next ones. Of the
    /// earlier turns it keeps those that the turn's newest request carried: a turn that
    /// a summary covers, or that the history left out, is never sent again.
    fn remember(&mut self, end: TurnEnd) {
        if let Some(carried) = &end.carried {
            self.cache.retain(|turn_id, _| carried.contains(turn_id));
        }
        if let Some(cached) = end.cached {
            self.cache.insert(end.turn_id, Arc::new(cached));
        }
    }

    async fn append(
        &self,
        events: Vec<Event>,
        receipt: NewReceipt,
    ) -> Result<Committed, ConversationError> {
        let conversation_id = self.conversation_id();
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(conversation_id, event))
            .receipt(receipt);
        self.shared.deps.writer.append(batch).await.map_err(ConversationError::from_store)
    }
}

/// The end of the running manual compaction; never, when none runs.
async fn compaction_end(
    compacting: &mut Option<Compacting>,
) -> Result<compact::Compacted, JoinError> {
    match compacting {
        Some(compacting) => (&mut compacting.task).await,
        None => std::future::pending().await,
    }
}

/// The end of the running turn; never, when none runs.
async fn turn_end(running: &mut Option<Running>) -> Result<TurnEnd, JoinError> {
    match running {
        Some(running) => (&mut running.task).await,
        None => std::future::pending().await,
    }
}

/// The receipt of a command whose result is `result`, stored without its sequence
/// numbers: the store records the number of one event with the receipt, and
/// [`completed_result`] puts them all back.
fn receipt<T: Serialize>(
    command_id: CommandId,
    method: &'static str,
    result: &T,
) -> Result<NewReceipt, ConversationError> {
    let mut value = serde_json::to_value(result)
        .map_err(|source| ConversationError::EncodeReceipt { method, source })?;
    if let Value::Object(members) = &mut value {
        members.remove("seq");
        for nested in [RESENT, WITHDRAWN] {
            match members.get_mut(nested) {
                Some(Value::Object(item)) => {
                    item.remove("seq");
                }
                Some(Value::Array(items)) => {
                    for item in items.iter_mut().filter_map(Value::as_object_mut) {
                        item.remove("seq");
                    }
                }
                _ => {}
            }
        }
    }
    Ok(NewReceipt::accepted(command_id, method, value))
}

/// The member of a `turn.interrupt` result that names the resent steers.
const RESENT: &str = "resent";
/// The member of a `turn.interrupt` or `prompt.withdraw` result that names the
/// withdrawn prompts.
const WITHDRAWN: &str = "withdrawn";
/// The member of a `turn.interrupt` result that names the steers taken back.
const WITHDRAWN_STEERS: &str = "withdrawn_steers";

/// The first answer of a command of `method`, from the result that its receipt stored
/// and the receipt's sequence number `seq`.
///
/// A stored result has no sequence numbers. The receipt keeps the number of one event
/// of its batch, and the events of a batch have consecutive numbers, so the others
/// follow from where the actor put them: a `turn.interrupt` batch holds
/// `turn_interrupt_requested` (`seq`), then one `prompt_withdrawn` per withdrawn prompt,
/// then `steering_withdrawn` when steers were taken back, then the `prompt_queued` of
/// the resent steers; `prompt.withdraw` reports only its
/// `prompt_withdrawn` (`seq`), in `withdrawn`; every other result reports `seq` at the
/// top.
pub fn completed_result(method: &str, mut result: Value, seq: Seq) -> Value {
    let Value::Object(members) = &mut result else {
        return result;
    };
    let number = |offset: usize| Value::from(seq.get().saturating_add(offset as u64));
    if method == PROMPT_WITHDRAW {
        if let Some(Value::Object(withdrawn)) = members.get_mut(WITHDRAWN) {
            withdrawn.insert("seq".to_owned(), number(0));
        }
        return result;
    }
    members.insert("seq".to_owned(), number(0));
    if method == TURN_INTERRUPT {
        let mut count = 0;
        if let Some(Value::Array(withdrawn)) = members.get_mut(WITHDRAWN) {
            for item in withdrawn.iter_mut().filter_map(Value::as_object_mut) {
                count += 1;
                item.insert("seq".to_owned(), number(count));
            }
        }
        // NOTE: `steering_withdrawn` stands between, and the members of
        // `withdrawn_steers` keep the seqs of their `turn_steered` events.
        if matches!(members.get(WITHDRAWN_STEERS), Some(Value::Array(steers)) if !steers.is_empty())
        {
            count += 1;
        }
        if let Some(Value::Object(resent)) = members.get_mut(RESENT) {
            resent.insert("seq".to_owned(), number(count + 1));
        }
    }
    result
}

/// The sequence number of the batch's event at `index`.
fn seq_of(committed: &Committed, index: usize) -> Seq {
    committed.events().get(index).map_or(committed.last_seq(), |envelope| envelope.seq)
}

impl ConversationHandle {
    /// The conversation.
    pub fn id(&self) -> ConversationId {
        self.conversation_id
    }

    /// The status drafts of the running turn, for a client that attaches while it
    /// runs: its newest `context` draft, then its `compacting` draft while it compacts.
    /// Empty when no turn runs or the turn has sent neither yet.
    pub fn live_drafts(&self) -> Vec<ConversationDraft> {
        self.status.drafts()
    }

    /// Sends a prompt. It runs at once when the conversation is idle and queues behind
    /// the running turn otherwise. The params' `conversation_id` and
    /// `new_conversation` are the daemon's routing and are only checked here; the
    /// last command reaches the turn's preamble in memory and is never recorded.
    pub async fn send_prompt(
        &self,
        params: PromptSend,
        origin: Origin,
    ) -> Result<PromptSendResult, ConversationError> {
        self.request(|reply| Request::Prompt { params: Box::new(params), origin, reply }).await
    }

    /// Adds guidance to the running turn; the model reads it at its next step. A late
    /// steer, one that no model call of the turn would read, is refused with
    /// `NoRunningTurn` or `TurnMismatch`, or becomes a queued prompt from `origin` when
    /// the params' `if_late` asks for it.
    pub async fn steer(
        &self,
        params: TurnSteer,
        origin: Origin,
    ) -> Result<TurnSteerResult, ConversationError> {
        self.request(|reply| Request::Steer { params, origin, reply }).await
    }

    /// Takes a queued prompt back before it starts and records `prompt_withdrawn`. A
    /// prompt that started, ended or was withdrawn is `PromptNotWaiting`; a turn that
    /// this conversation never queued is `UnknownTurn`, and a terminal without a queued
    /// prompt is `NoQueuedPrompt`.
    pub async fn withdraw(
        &self,
        params: PromptWithdraw,
        origin: Origin,
    ) -> Result<PromptWithdrawResult, ConversationError> {
        self.request(|reply| Request::Withdraw { params, origin, reply }).await
    }

    /// Asks the running turn to stop. The request is recorded at once; the turn records
    /// `turn_interrupted` when the model's stream has actually stopped. In the same
    /// append, it withdraws the queued prompts that the params' `withdraw` lists, takes
    /// back the unread steers that `withdraw_steers` lists, and sends the unread steers
    /// that `resend_steers` lists again as one prompt that runs next, with the values of
    /// `resend_as` when it is given.
    pub async fn interrupt(
        &self,
        params: TurnInterrupt,
        origin: Origin,
    ) -> Result<TurnInterruptResult, ConversationError> {
        self.request(|reply| Request::Interrupt { params, origin, reply }).await
    }

    /// Answers the approval request of a parked tool call.
    pub async fn respond_approval(
        &self,
        params: ApprovalRespond,
        origin: Origin,
    ) -> Result<ApprovalRespondResult, ConversationError> {
        self.request(|reply| Request::Approval { params, origin, reply }).await
    }

    /// Answers the quarantine question of a parked turn: `keep` moves the changes back.
    /// Only the user's own machine answers; a phone gets an error and the question
    /// stays open.
    pub async fn respond_surface(
        &self,
        params: SandboxSurfaceRespond,
        origin: Origin,
    ) -> Result<SandboxSurfaceRespondResult, ConversationError> {
        self.request(|reply| Request::Surface { params, origin, reply }).await
    }

    /// Compacts the conversation's context now: prunes and writes a summary of the
    /// history before the verbatim tail, records `conversation_compacted` with the
    /// command's receipt, and answers when it is done. It never starts a turn. While a
    /// turn or another compaction runs it is refused with `CompactionBusy`; with nothing
    /// before the tail, with `NothingToCompact`. A prompt that arrives meanwhile queues
    /// behind it.
    pub async fn compact(
        &self,
        params: ConversationCompact,
    ) -> Result<ConversationCompactResult, ConversationError> {
        self.request(|reply| Request::Compact { params, reply }).await
    }

    /// What the conversation is doing.
    pub async fn state(&self) -> Result<ConversationState, ConversationError> {
        let (reply, answer) = oneshot::channel();
        self.send(Message::Request(Box::new(Request::State { reply }))).await?;
        answer.await.map_err(|_| ConversationError::Stopped)
    }

    /// Stops the actor and waits until it has. A running turn is dropped where it is;
    /// the reconciliation at the next daemon start cancels it.
    pub async fn shutdown(&self) -> Result<(), ConversationError> {
        let (reply, answer) = oneshot::channel();
        self.send(Message::Shutdown(reply)).await?;
        answer.await.map_err(|_| ConversationError::Stopped)
    }

    /// True once the actor has stopped.
    pub fn is_closed(&self) -> bool {
        self.mailbox.is_closed()
    }

    async fn request<T>(
        &self,
        make: impl FnOnce(Reply<T>) -> Request,
    ) -> Result<T, ConversationError> {
        let (reply, answer) = oneshot::channel();
        self.send(Message::Request(Box::new(make(reply)))).await?;
        answer.await.map_err(|_| ConversationError::Stopped)?
    }

    async fn send(&self, message: Message) -> Result<(), ConversationError> {
        self.mailbox.send(message).await.map_err(|_| ConversationError::Stopped)
    }
}

#[cfg(test)]
mod tests;
