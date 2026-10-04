//! One actor per conversation: the prompt queue and the turns.
//!
//! [`ConversationActor::spawn`] starts a task that owns the conversation's state and
//! returns its [`ConversationHandle`], the only way in. Requests arrive on a bounded
//! mailbox and are answered on a `oneshot`. A turn runs in a task of its own, so the
//! actor keeps answering (steering, interrupts, approvals, more prompts) while the
//! model streams. Prompts that arrive while a turn runs queue behind it, one turn at a
//! time.
//!
//! Every request that changes the conversation records its event and the command's
//! receipt in one batch, so a retried command id returns the stored result and nothing
//! runs twice. A result that reports a sequence number is stored without it; the store
//! records the number with the receipt, and the daemon completes the result from it
//! when it answers a retry.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use efr_protocol::{
    ApprovalRespond, ApprovalRespondResult, CallId, CommandId, ConversationId, ErrorBody,
    ErrorCode, Event, Origin, PromptSend, PromptSendResult, Seq, TurnId, TurnInterrupt,
    TurnInterruptResult, TurnSteer, TurnSteerResult,
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
use crate::history::CachedTurn;
use crate::scratch::Scratch;
use crate::turn::{self, Control, Shared, TurnEnd, TurnSpec};
use crate::{ConversationConfig, ConversationDeps, ConversationError, ConversationStart};

/// Requests waiting for the actor; senders wait when it is full.
const MAILBOX: usize = 64;

const PROMPT_SEND: &str = "prompt.send";
const TURN_STEER: &str = "turn.steer";
const TURN_INTERRUPT: &str = "turn.interrupt";
const APPROVAL_RESPOND: &str = "approval.respond";

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
    /// The exact messages of the newest turns this actor ran, for their provider items.
    cache: HashMap<TurnId, Arc<CachedTurn>>,
    cache_order: VecDeque<TurnId>,
}

/// A cheap, cloneable handle to a conversation's actor.
#[derive(Debug, Clone)]
pub struct ConversationHandle {
    conversation_id: ConversationId,
    mailbox: mpsc::Sender<Message>,
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
}

#[derive(Debug)]
struct Running {
    turn_id: TurnId,
    task: JoinHandle<TurnEnd>,
    control: Control,
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
    Prompt { params: PromptSend, origin: Origin, reply: Reply<PromptSendResult> },
    Steer { params: TurnSteer, reply: Reply<TurnSteerResult> },
    Interrupt { params: TurnInterrupt, origin: Origin, reply: Reply<TurnInterruptResult> },
    Approval { params: ApprovalRespond, origin: Origin, reply: Reply<ApprovalRespondResult> },
    State { reply: oneshot::Sender<ConversationState> },
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
        config: ConversationConfig,
        deps: ConversationDeps,
    ) -> ConversationHandle {
        let (sender, mailbox) = mpsc::channel(MAILBOX);
        let scratch = Mutex::new(Scratch::new(&config.scratch_root, conversation_id));
        let shared = Arc::new(Shared {
            conversation_id,
            config,
            deps,
            scratch,
            approvals: Approvals::default(),
        });
        let actor = ConversationActor {
            shared,
            mailbox,
            start,
            queue: VecDeque::new(),
            running: None,
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
        };
        let span = tracing::info_span!("conversation", conversation_id = %conversation_id);
        tokio::spawn(actor.run().instrument(span));
        ConversationHandle { conversation_id, mailbox: sender }
    }

    async fn run(mut self) {
        let mut open = true;
        let mut stopped_reply = None;
        while open || self.running.is_some() {
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
            }
        }
        if let Some(running) = self.running.take() {
            // NOTE: the turn's events stop where it was; the reconciliation at the next
            // daemon start cancels the turn and expires its approvals.
            running.task.abort();
            // Waiting for the cancelled task means nothing of the turn runs once the
            // shutdown is answered.
            let _ = running.task.await;
        }
        if let Some(reply) = stopped_reply {
            let _ = reply.send(());
        }
    }

    async fn handle(&mut self, request: Request) {
        // A caller that stopped waiting does not need the answer; the work is done.
        match request {
            Request::Prompt { params, origin, reply } => {
                let _ = reply.send(self.prompt(params, origin).await);
            }
            Request::Steer { params, reply } => {
                let _ = reply.send(self.steer(params).await);
            }
            Request::Interrupt { params, origin, reply } => {
                let _ = reply.send(self.interrupt(params, origin).await);
            }
            Request::Approval { params, origin, reply } => {
                let _ = reply.send(self.respond(params, origin).await);
            }
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
        let limit = self.shared.config.max_queued;
        if self.queue.len() >= limit {
            return Err(ConversationError::QueueFull {
                conversation_id: self.conversation_id(),
                limit,
            });
        }
        let turn_id = TurnId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng));
        let queued = self.running.is_some() || !self.queue.is_empty();
        let mut events = Vec::with_capacity(2);
        if let ConversationStart::New { origin, tty } = &self.start {
            events.push(Event::ConversationCreated { origin: *origin, tty: tty.clone() });
        }
        let index = events.len();
        events.push(Event::PromptQueued {
            turn_id,
            command_id: params.command_id,
            text: params.text.clone(),
            origin,
            context: params.context.clone(),
        });
        let result = PromptSendResult {
            conversation_id: self.conversation_id(),
            turn_id,
            seq: Seq::ZERO,
            queued,
        };
        let receipt = receipt(params.command_id, PROMPT_SEND, &result)?.seq_of_event(index);
        let committed = self.append(events, receipt).await?;
        self.start = ConversationStart::Existing;
        self.queue.push_back(TurnSpec {
            turn_id,
            text: params.text,
            origin,
            context: params.context,
            last_command: params.last_command,
        });
        self.start_next();
        Ok(PromptSendResult { seq: seq_of(&committed, index), ..result })
    }

    async fn steer(&mut self, params: TurnSteer) -> Result<TurnSteerResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let turn_id = self.running_turn(params.turn_id)?;
        let event = Event::TurnSteered { turn_id, text: params.text.clone() };
        let result = TurnSteerResult { turn_id, seq: Seq::ZERO };
        let receipt = receipt(params.command_id, TURN_STEER, &result)?;
        let committed = self.append(vec![event], receipt).await?;
        if let Some(running) = &self.running {
            running.control.steering.push(params.text);
        }
        Ok(TurnSteerResult { seq: committed.last_seq(), ..result })
    }

    async fn interrupt(
        &mut self,
        params: TurnInterrupt,
        origin: Origin,
    ) -> Result<TurnInterruptResult, ConversationError> {
        self.check_conversation(Some(params.conversation_id))?;
        let turn_id = self.running_turn(params.turn_id)?;
        let event = Event::TurnInterruptRequested { turn_id, origin };
        let result = TurnInterruptResult { turn_id, seq: Seq::ZERO };
        let receipt = receipt(params.command_id, TURN_INTERRUPT, &result)?;
        let committed = self.append(vec![event], receipt).await?;
        if let Some(running) = &self.running {
            running.control.interrupt.raise();
        }
        Ok(TurnInterruptResult { seq: committed.last_seq(), ..result })
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

    fn state(&self) -> ConversationState {
        ConversationState {
            running: self.running.as_ref().map(|running| running.turn_id),
            queued: self.queue.iter().map(|spec| spec.turn_id).collect(),
            pending_approvals: self.shared.approvals.parked(),
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
        if self.running.is_some() {
            return;
        }
        let Some(spec) = self.queue.pop_front() else {
            return;
        };
        let turn_id = spec.turn_id;
        let control = Control::new();
        let turn = turn::run(Arc::clone(&self.shared), spec, control.clone(), self.cache.clone());
        let task = tokio::spawn(turn.in_current_span());
        self.running = Some(Running { turn_id, task, control });
    }

    async fn turn_ended(&mut self, ended: Result<TurnEnd, JoinError>) {
        let Some(running) = self.running.take() else {
            return;
        };
        if running.control.steering.is_waiting() {
            // NOTE: steering that arrives after the turn's last model call is recorded
            // but never answered; it is rare enough to log rather than start a turn.
            tracing::warn!(turn_id = %running.turn_id, "steering arrived after the turn's last model call");
        }
        match ended {
            Ok(end) => self.remember(end),
            Err(error) => self.turn_lost(running.turn_id, &error).await,
        }
        self.start_next();
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
        events.push(Event::TurnFailed { turn_id, error });
        let batch = events
            .into_iter()
            .fold(Batch::new(), |batch, event| batch.event(self.conversation_id(), event));
        if let Err(error) = self.shared.deps.writer.append(batch).await {
            tracing::error!(error = %error, "the end of a lost turn could not be recorded");
        }
    }

    /// Keeps the messages of a finished turn for the history of the next ones, as many
    /// turns as the history may carry.
    fn remember(&mut self, end: TurnEnd) {
        let Some(cached) = end.cached else {
            return;
        };
        self.cache.insert(end.turn_id, Arc::new(cached));
        self.cache_order.push_back(end.turn_id);
        while self.cache_order.len() > self.shared.config.history.max_turns {
            if let Some(oldest) = self.cache_order.pop_front() {
                self.cache.remove(&oldest);
            }
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

/// The end of the running turn; never, when none runs.
async fn turn_end(running: &mut Option<Running>) -> Result<TurnEnd, JoinError> {
    match running {
        Some(running) => (&mut running.task).await,
        None => std::future::pending().await,
    }
}

/// The receipt of a command whose result is `result`, stored without its `seq`, which
/// the store records with the receipt.
fn receipt<T: Serialize>(
    command_id: CommandId,
    method: &'static str,
    result: &T,
) -> Result<NewReceipt, ConversationError> {
    let mut value = serde_json::to_value(result)
        .map_err(|source| ConversationError::EncodeReceipt { method, source })?;
    if let Value::Object(members) = &mut value {
        members.remove("seq");
    }
    Ok(NewReceipt::accepted(command_id, method, value))
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

    /// Sends a prompt. It runs at once when the conversation is idle and queues behind
    /// the running turn otherwise. The params' `conversation_id` and
    /// `new_conversation` are the daemon's routing and are only checked here; the
    /// last command reaches the turn's preamble in memory and is never recorded.
    pub async fn send_prompt(
        &self,
        params: PromptSend,
        origin: Origin,
    ) -> Result<PromptSendResult, ConversationError> {
        self.request(|reply| Request::Prompt { params, origin, reply }).await
    }

    /// Adds guidance to the running turn; the model reads it at its next step.
    pub async fn steer(&self, params: TurnSteer) -> Result<TurnSteerResult, ConversationError> {
        self.request(|reply| Request::Steer { params, reply }).await
    }

    /// Asks the running turn to stop. The request is recorded at once; the turn records
    /// `turn_interrupted` when the model's stream has actually stopped.
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
