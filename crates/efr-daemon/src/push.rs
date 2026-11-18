//! Pushes to the paired phones through the Expo push service (ADR 0028): the rule of
//! when to push, the repeats, and the open tickets.
//!
//! efrd sends JSON to the Expo push service (`efr_http::ExpoPush`), which sends the
//! notice to Apple with the push key that EAS holds for the app. efrd keeps no key and
//! no secret for push; the Expo push token of each phone is in the store
//! (`efr_store::device_push`).
//!
//! The rule (decision D3). A phone gets a push when all of these are true:
//!
//! 1. Something needs you ([`Row`]): a row that `notices::desktop_what` gives (an
//!    approval or a question of the model, the end, the fail or the wake limit of a job
//!    notice turn, a command that waits for input), or the end of a turn that a phone
//!    started and that took at least `notify.after_secs`.
//! 2. `presence.push` is on.
//! 3. You are not at the terminal of that conversation: it is away, or its newest input
//!    came from a phone (`PresenceTable::not_at_terminal`).
//! 4. No connection of that phone holds a visible lease (`Connections::visible_on`).
//! 5. The phone has a token and proved its key in the last [`SEEN_WITHIN`].
//!
//! Two callers hand a row over ([`PushQueue`]): the notices follower, when its event
//! commits, and a turn whose question finds nobody there later (`PresenceReader::push`).
//! [`serve`] takes the rows one after the other and sends each one on a task of its
//! own, so a slow answer of Expo never holds a notice or a turn. A row that the rule
//! lets through goes out once ([`Repeats`]); one that it stops may come again, such
//! as when you turn away while the question still waits.
//!
//! The body follows the privacy rule of the desktop notice: the title `efr` and
//! `notice_body(place, what)`, such as `app: a question waits`, never a prompt, a
//! command, a title or text of the model. The `data` of a push holds only the daemon id
//! and the conversation id, which a tap needs.
//!
//! Each ticket that Expo gives waits for its receipt ([`OpenTickets`]); one timer on
//! the clock wakes when the oldest is due, and none runs while no ticket waits.
//! `DeviceNotRegistered`, in a ticket or a receipt, deletes the token of the device
//! when the device still has that token, so push is off for it until the app sends a
//! token again. `InvalidCredentials` gives one warning for each start of efrd; each
//! other code gives one log line with the code and the device id. No log holds a
//! token: a device shows in a log by its id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_http::{
    DEVICE_NOT_REGISTERED, EXPO_PUSH_BASE, ExpoAnswer, ExpoPush, HttpClient, INVALID_CREDENTIALS,
    MAX_MESSAGES, PushCode, PushMessage, Receipt, TOO_MANY_EXPERIENCE_IDS, Ticket, TicketId,
};
use efr_protocol::{CallId, ConversationId, DeviceId, Event, EventEnvelope, Origin};
use efr_stdx::rng::Rng;
use efr_stdx::text::{QUESTION_WAITS, notice_body, notice_ended};
use efr_stdx::time::Clock;
use efr_store::device_push::PushTarget;
use efr_transport::ConnectionContext;
use jiff::Timestamp;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::notices::{self, Wake};
use crate::state::State;

mod check;
mod repeats;
mod tickets;

pub(crate) use check::check;
use repeats::{Repeats, Subject};
use tickets::OpenTickets;

/// How recently a phone must have proved its key to get a push. A phone that was not
/// revoked gets no push after this long without a connection.
const SEEN_WITHIN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long Apple keeps a notice that is not a question for a phone that is off. A
/// question lasts `presence.away_wait_secs`, as long as it waits.
const NOTICE_TTL: Duration = Duration::from_secs(60 * 60);

/// The rows that wait for the push task. A row is small, and a full queue drops the
/// newest with a warning.
const QUEUE_CAPACITY: usize = 64;

/// The push service of efrd.
#[derive(Debug)]
pub(crate) struct Push {
    /// The client of the Expo push service; `None` when `EFR_EXPO_PUSH_URL` is not an
    /// `http` or `https` URL, and efrd sends no push.
    expo: Option<ExpoPush>,
    settings: watch::Receiver<Arc<Settings>>,
    queue: PushQueue,
    /// The other end of `queue`, until [`serve`] takes it.
    rows: Mutex<Option<mpsc::Receiver<Row>>>,
    memory: Mutex<Memory>,
    /// True once `InvalidCredentials` was logged at this start.
    warned_credentials: AtomicBool,
}

/// What the push task remembers between two pushes.
#[derive(Debug, Default)]
struct Memory {
    repeats: Repeats,
    open: OpenTickets,
}

impl Push {
    /// The push service that sends to `endpoint`, the base URL of the Expo push service
    /// (`EFR_EXPO_PUSH_URL`, else `https://exp.host`), over `http`, the shared client
    /// of efrd. It reads `presence.push` from `settings`; its tries wait on `clock`
    /// and draw their jitter from `rng`.
    pub(crate) fn new(
        endpoint: Option<&str>,
        http: HttpClient,
        settings: watch::Receiver<Arc<Settings>>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        let expo = match ExpoPush::new(endpoint.unwrap_or(EXPO_PUSH_BASE), http, clock, rng) {
            Ok(expo) => Some(expo),
            Err(error) => {
                tracing::warn!(error = %error, "EFR_EXPO_PUSH_URL is not an http or https URL; efrd sends no push");
                None
            }
        };
        let (queue, rows) = PushQueue::channel();
        Push {
            expo,
            settings,
            queue,
            rows: Mutex::new(Some(rows)),
            memory: Mutex::default(),
            warned_credentials: AtomicBool::new(false),
        }
    }

    /// The capability `push` of the connection of `context`: true on the tailnet
    /// listener while `presence.push` is on, else `None`.
    pub(crate) fn capability(&self, context: &ConnectionContext) -> Option<bool> {
        let on = self.expo.is_some() && self.settings.borrow().presence.push;
        (on && context.remote().is_some()).then_some(true)
    }

    /// Where a row goes to the push task.
    pub(crate) fn queue(&self) -> &PushQueue {
        &self.queue
    }

    /// True for the first push about `subject` of `conversation`.
    fn first(&self, conversation: ConversationId, subject: Subject) -> bool {
        self.memory().repeats.first(conversation, subject)
    }

    fn memory(&self) -> MutexGuard<'_, Memory> {
        // Every critical section leaves the memory whole, so a poisoned lock is still
        // good.
        self.memory.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where the notices follower and the turns hand over a [`Row`]. It never waits: a full
/// queue drops the row.
#[derive(Debug, Clone)]
pub(crate) struct PushQueue(mpsc::Sender<Row>);

impl PushQueue {
    /// A queue and the receiver of its rows.
    pub(crate) fn channel() -> (PushQueue, mpsc::Receiver<Row>) {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        (PushQueue(sender), receiver)
    }

    /// Hands `row` to the push task. A closed queue means that efrd sends no push.
    pub(crate) fn hand_over(&self, row: Row) {
        if let Err(mpsc::error::TrySendError::Full(row)) = self.0.try_send(row) {
            tracing::warn!(conversation_id = %row.conversation, "the push queue is full; a push was dropped");
        }
    }
}

/// Something that needs you in a conversation: rule 1 of the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    conversation: ConversationId,
    subject: Subject,
    what: What,
}

/// What a [`Row`] tells, for the body of its push.
#[derive(Debug, Clone, PartialEq, Eq)]
enum What {
    /// A row that waits for you, with the `<what>` of the body. A question (an
    /// approval or a question of the model) lasts as long as it waits.
    Waits { what: &'static str, question: bool },
    /// The end of a turn that a phone started.
    Ended { took: Duration, failed: bool },
}

impl Row {
    /// The question of `call_id` in `conversation`, which waits with nobody there.
    pub(crate) fn question(conversation: ConversationId, call_id: CallId) -> Self {
        let what = What::Waits { what: QUESTION_WAITS, question: true };
        Row { conversation, subject: Subject::Call(call_id), what }
    }

    /// The body of the push at `place`.
    fn body(&self, place: &str) -> String {
        match self.what {
            What::Waits { what, .. } => notice_body(place, what),
            What::Ended { took, failed } => notice_ended(place, took, failed),
        }
    }

    /// How long Apple keeps the push: a question as long as it waits, else
    /// [`NOTICE_TTL`].
    fn ttl(&self, settings: &Settings) -> Duration {
        match self.what {
            What::Waits { question: true, .. } => {
                Duration::from_secs(u64::from(settings.presence.away_wait_secs))
            }
            _ => NOTICE_TTL,
        }
    }
}

/// The row of `envelope` in `conversation_id` that needs you, when it has one and
/// `presence.push` is on. `wake` is the job notice turn that a `turn_completed` or a
/// `turn_failed` ends. The end of another turn reads the store: its row is there only
/// for a turn that a phone started and that took at least `notify.after_secs`.
pub(crate) async fn row(
    state: &State,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
    wake: Option<&Wake>,
) -> Option<Row> {
    let settings = Arc::clone(&state.settings.borrow());
    if state.push.expo.is_none() || !settings.presence.push {
        return None;
    }
    if let Some(row) = waiting(conversation_id, envelope, wake) {
        return Some(row);
    }
    let (turn_id, failed) = match &envelope.event {
        Event::TurnCompleted { turn_id, .. } => (*turn_id, false),
        Event::TurnFailed { turn_id, .. } => (*turn_id, true),
        _ => return None,
    };
    let took = phone_turn(state, conversation_id, turn_id).await?;
    let after = Duration::from_secs(u64::from(settings.notify.after_secs));
    let what = What::Ended { took, failed };
    (took >= after).then_some(Row {
        conversation: conversation_id,
        subject: Subject::Turn(turn_id),
        what,
    })
}

/// The row of `envelope` that the desktop notice also gives (`notices::desktop_what`),
/// with no read of the store.
fn waiting(
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
    wake: Option<&Wake>,
) -> Option<Row> {
    let event = &envelope.event;
    let what = notices::desktop_what(event, wake)?;
    let question =
        matches!(event, Event::ApprovalRequested { .. } | Event::UserQuestionRequested { .. });
    let subject = match event {
        Event::ApprovalRequested { call_id, .. }
        | Event::UserQuestionRequested { call_id, .. }
        | Event::ShellCommandDetached { call_id, .. }
        | Event::ShellCommandInputChanged { call_id, .. } => Subject::Call(*call_id),
        Event::TurnCompleted { turn_id, .. } | Event::TurnFailed { turn_id, .. } => {
            Subject::Turn(*turn_id)
        }
        _ => Subject::Event(envelope.seq),
    };
    Some(Row { conversation: conversation_id, subject, what: What::Waits { what, question } })
}

/// How long the turn `turn_id` ran, when a phone sent its prompt; `None` for a turn of
/// another origin, or one that the store does not have.
async fn phone_turn(
    state: &State,
    conversation_id: ConversationId,
    turn_id: efr_protocol::TurnId,
) -> Option<Duration> {
    let read = state
        .readers
        .with(move |conn| {
            let Some(turn) = efr_store::conversations::turn(conn, turn_id)? else {
                return Ok(None);
            };
            let queued = turn.queued_seq;
            let prompt = efr_store::events::read_turn_range(conn, conversation_id, queued, queued)?;
            let phone = prompt.iter().any(|envelope| {
                matches!(envelope.event, Event::PromptQueued { origin: Origin::Phone, .. })
            });
            Ok(phone.then_some((turn.started_at, turn.ended_at)))
        })
        .await;
    match read {
        Ok(Some((Some(started), Some(ended)))) => {
            Some(Duration::try_from(ended.duration_since(started)).unwrap_or_default())
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(error = %error, %conversation_id, "the turn of a push could not be read");
            None
        }
    }
}

/// Takes the rows of the queue and sends each one on a task of its own, and reads the
/// receipts of the open tickets when they are due, until `stop`.
pub(crate) async fn serve(state: Arc<State>, stop: CancellationToken) {
    let Some(mut rows) = state.push.rows.lock().unwrap_or_else(PoisonError::into_inner).take()
    else {
        return;
    };
    let Some(expo) = state.push.expo.clone() else {
        return;
    };
    // NOTE: the sends and the reads run beside the loop, and a stop drops them with the
    // set, so no task holds the state after efrd stops.
    let mut work = JoinSet::new();
    loop {
        let due = state.push.memory().open.next_due();
        let wait = due.map(|due| until(state.clock.now(), due));
        tokio::select! {
            () = stop.cancelled() => return,
            row = rows.recv() => {
                let Some(row) = row else { return };
                let (state, expo) = (Arc::clone(&state), expo.clone());
                work.spawn(async move { deliver(&state, &expo, row).await });
            }
            () = sleep_for(&*state.clock, wait) => {
                let due = state.push.memory().open.take_due(state.clock.now());
                let (state, expo) = (Arc::clone(&state), expo.clone());
                work.spawn(async move { read_receipts(&state, &expo, due).await });
            }
            Some(_) = work.join_next(), if !work.is_empty() => {}
        }
    }
}

/// The time from `now` to `due`, none when it passed.
fn until(now: Timestamp, due: Timestamp) -> Duration {
    Duration::try_from(due.duration_since(now)).unwrap_or_default()
}

/// Sleeps `wait` on `clock`, and for ever without one.
async fn sleep_for(clock: &dyn Clock, wait: Option<Duration>) {
    match wait {
        Some(wait) => clock.sleep(wait).await,
        None => std::future::pending().await,
    }
}

/// Sends the push of `row` to each phone that rules 2 to 5 let it reach.
async fn deliver(state: &State, expo: &ExpoPush, row: Row) {
    let settings = Arc::clone(&state.settings.borrow());
    let conversation_id = row.conversation;
    if !settings.presence.push || !state.presence.not_at_terminal(conversation_id) {
        return;
    }
    let now = state.clock.now();
    let since = now.checked_sub(SEEN_WITHIN).unwrap_or(now);
    let read = state
        .readers
        .with(move |conn| {
            let targets = efr_store::device_push::targets(conn, Some(since))?;
            Ok((targets, efr_store::conversations::get(conn, conversation_id)?))
        })
        .await;
    let (targets, summary) = match read {
        Ok((targets, Some(summary))) => (targets, summary),
        Ok((_, None)) => return,
        Err(error) => {
            tracing::warn!(error = %error, %conversation_id, "the phones of a push could not be read");
            return;
        }
    };
    let targets: Vec<PushTarget> = targets
        .into_iter()
        .filter(|target| !state.connections.visible_on(target.device_id, now))
        .collect();
    if targets.is_empty() || !state.push.first(conversation_id, row.subject) {
        return;
    }
    let place = notices::place_of(state, summary.scope.as_ref(), summary.cwd.as_deref()).await;
    let (body, daemon) = (row.body(&place), state.daemon_id.to_string());
    let message = Message {
        body: &body,
        ttl: row.ttl(&settings),
        conversation: Some(conversation_id),
        daemon: &daemon,
    };
    let sent = send(expo, &targets, &message).await;
    for (target, sent) in targets.into_iter().zip(sent) {
        match sent {
            Sent::Ticket(id) => {
                state.push.memory().open.add(id, target.device_id, target.token, now);
            }
            Sent::Failed(code) => refused(state, target.device_id, target.token, code).await,
            Sent::Refused(_) | Sent::NoAnswer => {}
        }
    }
}

/// Reads the receipts of the tickets `due`, and asks once more for each one that has
/// none yet.
async fn read_receipts(state: &State, expo: &ExpoPush, due: Vec<tickets::OpenTicket>) {
    if due.is_empty() {
        return;
    }
    let ids: Vec<TicketId> = due.iter().map(|ticket| ticket.id.clone()).collect();
    let mut receipts: HashMap<TicketId, Receipt> = match expo.receipts(&ids).await {
        Ok(ExpoAnswer::Data(receipts)) => receipts.into_iter().collect(),
        Ok(ExpoAnswer::Refused { status, code }) => {
            let code = code.as_ref().map_or("none", PushCode::as_str);
            tracing::warn!(%status, code, "the Expo push service refused a read of receipts");
            HashMap::new()
        }
        Ok(_) => HashMap::new(),
        Err(error) => {
            tracing::warn!(error = %error, "the receipts of pushes could not be read");
            HashMap::new()
        }
    };
    let now = state.clock.now();
    for ticket in due {
        match receipts.remove(&ticket.id) {
            Some(Receipt::Failed(code)) => refused(state, ticket.device, ticket.token, code).await,
            Some(_) => {}
            None => state.push.memory().open.again(ticket, now),
        }
    }
}

/// What one push of a send became.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    /// Expo took it; its receipt comes later.
    Ticket(TicketId),
    /// Expo refused this one message, with its code.
    Failed(Option<PushCode>),
    /// Expo refused the whole request, with its code; the send logged it once.
    Refused(Option<PushCode>),
    /// No answer came after the tries; the send logged it once.
    NoAnswer,
}

/// What a push says, apart from its phone.
struct Message<'a> {
    body: &'a str,
    ttl: Duration,
    conversation: Option<ConversationId>,
    daemon: &'a str,
}

impl Message<'_> {
    fn to(&self, target: &PushTarget) -> PushMessage {
        PushMessage::new(
            target.token.expose_secret(),
            self.body.to_owned(),
            self.ttl,
            self.conversation.map(|id| id.to_string()),
            self.daemon.to_owned(),
        )
    }
}

/// Sends `message` to each of `targets`, at most [`MAX_MESSAGES`] in a request, and
/// returns what became of each push, in their order. A request that holds tokens of two
/// Expo projects goes again as one request for each message.
async fn send(expo: &ExpoPush, targets: &[PushTarget], message: &Message<'_>) -> Vec<Sent> {
    let mut sent = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(MAX_MESSAGES) {
        let messages: Vec<PushMessage> = chunk.iter().map(|target| message.to(target)).collect();
        match send_once(expo, &messages).await {
            Err(Some(code)) if code.as_str() == TOO_MANY_EXPERIENCE_IDS && messages.len() > 1 => {
                for one in messages.chunks(1) {
                    match send_once(expo, one).await {
                        Ok(mut alone) => sent.append(&mut alone),
                        Err(code) => sent.push(Sent::Refused(code)),
                    }
                }
            }
            Ok(mut tickets) => sent.append(&mut tickets),
            Err(code) => sent.extend(std::iter::repeat_n(Sent::Refused(code), messages.len())),
        }
    }
    sent
}

/// One request of `messages`: one [`Sent`] for each, or the code of a refusal of the
/// whole request (`Err(None)` with no code, or with no answer).
async fn send_once(
    expo: &ExpoPush,
    messages: &[PushMessage],
) -> Result<Vec<Sent>, Option<PushCode>> {
    match expo.send(messages).await {
        Ok(ExpoAnswer::Data(tickets)) => {
            let mut sent: Vec<Sent> = tickets
                .into_iter()
                .map(|ticket| match ticket {
                    Ticket::Ok(id) => Sent::Ticket(id),
                    Ticket::Failed(code) => Sent::Failed(code),
                    _ => Sent::Failed(None),
                })
                .collect();
            sent.resize(messages.len(), Sent::NoAnswer);
            Ok(sent)
        }
        Ok(ExpoAnswer::Refused { status, code }) => {
            let text = code.as_ref().map_or("none", PushCode::as_str);
            tracing::warn!(%status, code = text, "the Expo push service refused a push");
            Err(code)
        }
        Ok(_) => Err(None),
        Err(error) => {
            tracing::warn!(error = %error, "the Expo push service gave no answer");
            Ok(vec![Sent::NoAnswer; messages.len()])
        }
    }
}

/// What efrd does when Expo or Apple refused the push to `device` with `token`:
/// `DeviceNotRegistered` turns push off for the device while it still has `token`.
async fn refused(
    state: &State,
    device_id: DeviceId,
    token: efr_protocol::SecretText,
    code: Option<PushCode>,
) {
    match code.as_ref().map(PushCode::as_str) {
        Some(DEVICE_NOT_REGISTERED) => {
            match state.writer.device_push_drop(device_id, token).await {
                Ok(true) => {
                    tracing::info!(%device_id, "push is off for a device: Expo says that its token is dead")
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(error = %error, %device_id, "a dead push token could not be deleted")
                }
            }
        }
        Some(INVALID_CREDENTIALS) => {
            if !state.push.warned_credentials.swap(true, Ordering::Relaxed) {
                tracing::warn!("Expo has no valid push key for the app; run eas credentials");
            }
        }
        code => {
            tracing::warn!(%device_id, code = code.unwrap_or("none"), "a push did not reach the phone");
        }
    }
}

#[cfg(test)]
mod tests;
