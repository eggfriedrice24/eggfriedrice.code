//! What the daemon keeps per connection: the hello's tty and client, the open
//! conversation subscriptions (and which of them can type answers for a waiting
//! command, and which can answer the model's question), the prompts it sent, and the
//! lease of `lease.report`.
//!
//! The transport carries only `{surface, uid, pid, conn_id}` with each request; the rest
//! of the hello lives here from the accepted hello until `Dispatcher::closed`. The
//! notices use it to decide whether a terminal shows a conversation's event, and the
//! pushes whether the app of a phone is in front (`Connections::visible_on`).
//!
//! The notices decide after the commit, on a task of their own, and by then `efr` may
//! have shown the event, exited and closed its connection, or, for a turn that ended
//! at once, not have subscribed yet. So a terminal gets a notice only when none of its
//! subscriptions was handed the event, and the decision waits while a client in the
//! terminal may still show it: a subscription to the conversation that is open, or a
//! connection that sent the conversation a prompt (or is sending one) and is still
//! open. A held notice is decided again when the last of those ends, and goes to
//! [`Connections::take_ready`] if nobody was handed its event. Every count and every
//! high-water mark sits behind one lock, so no decision falls between a subscription's
//! end and the record of how far it got.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_protocol::{ConversationId, DeviceId, Origin, PtyId, Scope, Seq};
use efr_transport::ConnId;
use jiff::Timestamp;
use tokio::sync::Notify;

/// How long a lease lasts without another report. Clients report about every 25
/// seconds, so two missed reports end it.
pub(crate) const LEASE_TTL: Duration = Duration::from_secs(60);

/// The most terminal and conversation pairs that are remembered. The notices need an
/// ended pair only for the moments after its subscription ends, so the pair that
/// followed least far goes first; a pair with an open subscription or a held notice
/// stays.
const MAX_FOLLOWED: usize = 1024;

/// The most notices held for one terminal and conversation; the oldest goes first.
/// Notices are held only while a client in the terminal is about to show the event, so
/// the limit matters only for a view that stays open over many turns.
const MAX_HELD: usize = 64;

/// What a hello declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelloInfo {
    pub(crate) surface: Origin,
    pub(crate) tty: Option<String>,
    pub(crate) client: Option<String>,
}

/// The latest `lease.report` of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lease {
    pub(crate) conversations: Vec<ConversationId>,
    pub(crate) ptys: Vec<PtyId>,
    pub(crate) visible: bool,
    pub(crate) expires_at: Timestamp,
    /// The device of the proof, for a connection of the tailnet listener: while one of
    /// its leases is visible, nothing pushes to it.
    pub(crate) device: Option<DeviceId>,
}

/// A notice line for the terminal `tty` about the event `seq` of `conversation`, with
/// its desktop notice when the event needs the user. A held notice keeps its desktop
/// notice, so it sends the same one when it goes out later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) tty: String,
    pub(crate) conversation: ConversationId,
    pub(crate) seq: Seq,
    pub(crate) text: String,
    pub(crate) desktop: Option<Desktop>,
}

/// The desktop notice of a [`Notice`]. Only a notice that goes out gets its place
/// (`notices::place_of`), so a notice that a client shows reads no project registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Desktop {
    /// The `<what>` of the body.
    pub(crate) what: &'static str,
    /// The scope of the conversation, for the `<place>` of the body.
    pub(crate) scope: Option<Scope>,
    /// The working directory of the conversation, for the `<place>` of the body.
    pub(crate) cwd: Option<PathBuf>,
}

/// What [`Connections::decide`] made of a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// No client in the terminal shows the event or may still show it: write it.
    Write(Notice),
    /// A subscription from the terminal was handed the event, or a live lease from the
    /// terminal names the conversation: no notice.
    Shown,
    /// A client in the terminal may still show the event; the notice waits until it
    /// ends and then goes to [`Connections::take_ready`] unless it was shown.
    Held,
}

#[derive(Debug)]
struct Conn {
    hello: HelloInfo,
    /// Open subscriptions at which a person can type answers (`answers_input`), per
    /// conversation.
    answering: HashMap<ConversationId, usize>,
    /// Open subscriptions at which a person can answer the model's question
    /// (`answers_questions`), per conversation.
    questioning: HashMap<ConversationId, usize>,
    lease: Option<Lease>,
    /// `prompt.send` requests of this connection that have not answered yet. Their
    /// conversation is not known before they answer, and their turn may end before.
    prompting: usize,
    /// The conversations this connection sent a prompt to; `efr` follows the turn on
    /// the same connection right after.
    prompted: HashSet<ConversationId>,
}

/// How far one terminal followed one conversation.
#[derive(Debug, Default)]
struct Follow {
    /// The highest sequence number handed to each open subscription, raised by the
    /// subscription itself.
    open: Vec<Arc<AtomicU64>>,
    /// The highest sequence number that an ended subscription was handed.
    reached: Seq,
    /// Notices waiting for the open clients of the terminal to end, oldest first.
    held: Vec<Notice>,
}

impl Follow {
    /// True when a subscription, open or ended, was handed `seq` or a later event.
    fn handed(&self, seq: Seq) -> bool {
        self.reached >= seq
            || self.open.iter().any(|open| open.load(Ordering::Acquire) >= seq.get())
    }

    /// True when nothing about the pair needs keeping.
    fn is_empty(&self) -> bool {
        self.open.is_empty() && self.held.is_empty() && self.reached == Seq::ZERO
    }
}

#[derive(Debug, Default)]
struct Inner {
    conns: HashMap<ConnId, Conn>,
    /// Per terminal and conversation.
    follows: HashMap<(String, ConversationId), Follow>,
    /// Held notices whose event no subscription from their terminal was handed.
    ready: Vec<Notice>,
}

impl Inner {
    /// True while a client in `tty` may still show an event of `conversation`: an open
    /// subscription, or an open connection that sent it a prompt or is sending one.
    fn may_show(&self, tty: &str, conversation: ConversationId) -> bool {
        let key = (tty.to_owned(), conversation);
        self.follows.get(&key).is_some_and(|follow| !follow.open.is_empty())
            || self.conns.values().any(|conn| {
                conn.hello.tty.as_deref() == Some(tty)
                    && (conn.prompting > 0 || conn.prompted.contains(&conversation))
            })
    }

    /// Decides the held notices of `tty` whose clients have all ended; returns true
    /// when one became ready.
    fn settle(&mut self, tty: &str) -> bool {
        let keys: Vec<_> = self
            .follows
            .iter()
            .filter(|((held_tty, _), follow)| held_tty == tty && !follow.held.is_empty())
            .map(|(key, _)| key.clone())
            .collect();
        let mut readied = false;
        for key in keys {
            if self.may_show(&key.0, key.1) {
                continue;
            }
            let Some(follow) = self.follows.get_mut(&key) else {
                continue;
            };
            let reached = follow.reached;
            for notice in std::mem::take(&mut follow.held) {
                if notice.seq > reached {
                    self.ready.push(notice);
                    readied = true;
                }
            }
        }
        readied
    }

    /// Forgets the pair that followed least far while there are too many, never one
    /// with an open subscription or a held notice.
    fn trim(&mut self) {
        while self.follows.len() > MAX_FOLLOWED {
            let oldest = self
                .follows
                .iter()
                .filter(|(_, follow)| follow.open.is_empty() && follow.held.is_empty())
                .min_by_key(|(_, follow)| follow.reached)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.follows.remove(&key),
                None => return,
            };
        }
    }
}

/// Every connection with an accepted hello, and how far each terminal followed each
/// conversation.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    inner: Mutex<Inner>,
    /// Woken when a held notice becomes ready.
    wake: Notify,
}

impl Connections {
    /// Records the hello of `conn_id`.
    pub(crate) fn opened(&self, conn_id: ConnId, hello: HelloInfo) {
        let conn = Conn {
            hello,
            answering: HashMap::new(),
            questioning: HashMap::new(),
            lease: None,
            prompting: 0,
            prompted: HashSet::new(),
        };
        self.lock().conns.insert(conn_id, conn);
    }

    /// Forgets `conn_id`, its lease and its prompts, and decides the notices that
    /// waited for it.
    pub(crate) fn closed(&self, conn_id: ConnId) {
        let readied = {
            let mut inner = self.lock();
            let tty = inner.conns.remove(&conn_id).and_then(|conn| conn.hello.tty);
            tty.is_some_and(|tty| inner.settle(&tty))
        };
        if readied {
            self.wake.notify_one();
        }
    }

    /// The tty the hello of `conn_id` named.
    pub(crate) fn tty(&self, conn_id: ConnId) -> Option<String> {
        self.lock().conns.get(&conn_id).and_then(|conn| conn.hello.tty.clone())
    }

    /// The terminals with an open subscription to `conversation`: a terminal that
    /// follows a conversation holds it for routing (S-21). A subscription from a
    /// connection whose hello named no tty, such as the phone's, is not counted.
    pub(crate) fn followed_from(&self, conversation: ConversationId) -> Vec<String> {
        let mut ttys: Vec<String> = self
            .lock()
            .follows
            .iter()
            .filter(|((_, followed), follow)| *followed == conversation && !follow.open.is_empty())
            .map(|((tty, _), _)| tty.clone())
            .collect();
        ttys.sort();
        ttys
    }

    /// Records an open subscription of `conn_id` to `conversation` until the guard is
    /// dropped, which happens when the request ends however it ends. The subscription
    /// raises [`SubscriptionGuard::reached`] as it hands events over; when the guard
    /// drops, that sequence number is kept for the hello's terminal. `answers` tells
    /// what a person at the subscriber can answer: with `input`, the subscription
    /// counts in [`answerers`](Self::answerers), and with `questions`, in
    /// [`questioners`](Self::questioners), until the guard drops. A connection that the
    /// table does not know counts nowhere.
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        conn_id: ConnId,
        conversation: ConversationId,
        answers: Answers,
    ) -> SubscriptionGuard {
        let reached = Arc::new(AtomicU64::new(0));
        let mut inner = self.lock();
        let (tty, answers) = match inner.conns.get_mut(&conn_id) {
            Some(conn) => {
                if answers.input {
                    *conn.answering.entry(conversation).or_default() += 1;
                }
                if answers.questions {
                    *conn.questioning.entry(conversation).or_default() += 1;
                }
                (conn.hello.tty.clone(), answers)
            }
            None => (None, Answers::NONE),
        };
        if let Some(tty) = &tty {
            let follow = inner.follows.entry((tty.clone(), conversation)).or_default();
            follow.open.push(Arc::clone(&reached));
            inner.trim();
        }
        drop(inner);
        SubscriptionGuard {
            connections: Arc::clone(self),
            conn_id,
            conversation,
            tty,
            answers,
            reached,
        }
    }

    /// Counts a `prompt.send` of `conn_id` from now until the guard drops; the guard
    /// keeps the conversation the prompt went to with [`PromptGuard::sent_to`]. Until
    /// then a notice for the hello's terminal waits, because the prompt's turn may end
    /// before its client could subscribe to it.
    pub(crate) fn prompting(self: &Arc<Self>, conn_id: ConnId) -> PromptGuard {
        let counted = match self.lock().conns.get_mut(&conn_id) {
            Some(conn) => {
                conn.prompting += 1;
                true
            }
            None => false,
        };
        PromptGuard { connections: Arc::clone(self), conn_id, counted, sent_to: None }
    }

    /// How many live subscriptions of `conversation` can type answers for a command
    /// that waits for input. A command that waits for hidden input while there are none
    /// is stopped.
    pub(crate) fn answerers(&self, conversation: ConversationId) -> usize {
        self.count(|conn| conn.answering.get(&conversation))
    }

    /// How many live subscriptions of `conversation` can answer the model's question.
    /// While there are none, the question waits only the away wait.
    pub(crate) fn questioners(&self, conversation: ConversationId) -> usize {
        self.count(|conn| conn.questioning.get(&conversation))
    }

    /// The sum over the connections of the count that `of` finds in each.
    fn count(&self, of: impl Fn(&Conn) -> Option<&usize>) -> usize {
        self.lock().conns.values().filter_map(of).fold(0, |total, open| total.saturating_add(*open))
    }

    /// Replaces the lease of `conn_id`.
    pub(crate) fn report_lease(&self, conn_id: ConnId, lease: Lease) {
        if let Some(conn) = self.lock().conns.get_mut(&conn_id) {
            conn.lease = Some(lease);
        }
    }

    /// Decides whether `notice` reaches its terminal at `now`: not when a subscription
    /// from the terminal was handed its event or a live lease from it names the
    /// conversation; later when a client in it may still show the event; else now.
    pub(crate) fn decide(&self, notice: Notice, now: Timestamp) -> Decision {
        let mut inner = self.lock();
        let tty = notice.tty.as_str();
        let leased = inner.conns.values().any(|conn| {
            conn.hello.tty.as_deref() == Some(tty)
                && conn.lease.as_ref().is_some_and(|lease| {
                    lease.expires_at > now && lease.conversations.contains(&notice.conversation)
                })
        });
        let key = (notice.tty.clone(), notice.conversation);
        if leased || inner.follows.get(&key).is_some_and(|follow| follow.handed(notice.seq)) {
            return Decision::Shown;
        }
        if !inner.may_show(tty, notice.conversation) {
            return Decision::Write(notice);
        }
        let follow = inner.follows.entry(key).or_default();
        // NOTE: a view that stays open over many turns is handed each event soon after
        // its notice is held, so what it was handed by now goes first.
        let open = follow.open.iter().map(|open| open.load(Ordering::Acquire)).max();
        follow.held.retain(|held| open.is_none_or(|open| held.seq.get() > open));
        if follow.held.len() >= MAX_HELD {
            follow.held.remove(0);
        }
        follow.held.push(notice);
        inner.trim();
        Decision::Held
    }

    /// The held notices that became ready to write, oldest first.
    pub(crate) fn take_ready(&self) -> Vec<Notice> {
        std::mem::take(&mut self.lock().ready)
    }

    /// Completes once a held notice became ready since the last wait; a notice that
    /// became ready while nobody waited completes the next wait at once.
    pub(crate) fn readied(&self) -> impl Future<Output = ()> + '_ {
        self.wake.notified()
    }

    /// True when a connection of `device` holds a live lease that is visible: the app
    /// is in front.
    pub(crate) fn visible_on(&self, device: DeviceId, now: Timestamp) -> bool {
        self.lock().conns.values().any(|conn| {
            conn.lease.as_ref().is_some_and(|lease| {
                lease.device == Some(device) && lease.visible && lease.expires_at > now
            })
        })
    }

    /// True when a live lease names `pty_id`.
    pub(crate) fn watches_pty(&self, pty_id: PtyId, now: Timestamp) -> bool {
        self.lock().conns.values().any(|conn| {
            conn.lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now && lease.ptys.contains(&pty_id))
        })
    }

    /// Ends one subscription: keeps how far it got for its terminal and decides the
    /// notices that waited for it.
    fn unsubscribe(&self, guard: &SubscriptionGuard) {
        let readied = {
            let mut inner = self.lock();
            if let Some(conn) = inner.conns.get_mut(&guard.conn_id) {
                if guard.answers.input {
                    release(&mut conn.answering, guard.conversation);
                }
                if guard.answers.questions {
                    release(&mut conn.questioning, guard.conversation);
                }
            }
            match &guard.tty {
                Some(tty) => {
                    let key = (tty.clone(), guard.conversation);
                    if let Some(follow) = inner.follows.get_mut(&key) {
                        follow.open.retain(|open| !Arc::ptr_eq(open, &guard.reached));
                        let reached = Seq::new(guard.reached.load(Ordering::Acquire));
                        follow.reached = follow.reached.max(reached);
                        if follow.is_empty() {
                            inner.follows.remove(&key);
                        }
                    }
                    inner.settle(tty)
                }
                None => false,
            }
        };
        if readied {
            self.wake.notify_one();
        }
    }

    /// Ends one `prompt.send` of `conn_id`, which went to `sent_to` if it was sent.
    fn prompted(&self, conn_id: ConnId, sent_to: Option<ConversationId>) {
        let readied = {
            let mut inner = self.lock();
            let tty = match inner.conns.get_mut(&conn_id) {
                Some(conn) => {
                    conn.prompting = conn.prompting.saturating_sub(1);
                    if let Some(conversation) = sent_to {
                        conn.prompted.insert(conversation);
                    }
                    conn.hello.tty.clone()
                }
                None => None,
            };
            tty.is_some_and(|tty| inner.settle(&tty))
        };
        if readied {
            self.wake.notify_one();
        }
    }

    /// How many connections whose hello named `tty` are open; tests wait on it for the
    /// daemon to see a client leave.
    #[cfg(test)]
    pub(crate) fn open_in(&self, tty: &str) -> usize {
        self.lock().conns.values().filter(|conn| conn.hello.tty.as_deref() == Some(tty)).count()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Every critical section leaves the maps whole, so a poisoned lock is still good.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Takes one from the count of `conversation`, and forgets a count that reaches 0.
fn release(counts: &mut HashMap<ConversationId, usize>, conversation: ConversationId) {
    if let Some(open) = counts.get_mut(&conversation) {
        *open = open.saturating_sub(1);
        if *open == 0 {
            counts.remove(&conversation);
        }
    }
}

/// What a person at a subscriber can answer ([`Connections::subscribe`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Answers {
    /// Typed input for a command that waits for it.
    pub(crate) input: bool,
    /// The model's question.
    pub(crate) questions: bool,
}

impl Answers {
    /// A subscriber that answers nothing.
    pub(crate) const NONE: Self = Self { input: false, questions: false };
    /// A subscriber that types input for a waiting command only.
    #[cfg(test)]
    pub(crate) const INPUT: Self = Self { input: true, questions: false };
    /// A subscriber that answers the model's question only.
    #[cfg(test)]
    pub(crate) const QUESTIONS: Self = Self { input: false, questions: true };
}

/// An open subscription, counted until it is dropped.
#[derive(Debug)]
pub(crate) struct SubscriptionGuard {
    connections: Arc<Connections>,
    conn_id: ConnId,
    conversation: ConversationId,
    /// The hello's terminal, kept here because the connection may be forgotten before
    /// the request that holds this guard ends.
    tty: Option<String>,
    /// What the subscription counts as one that can answer.
    answers: Answers,
    reached: Arc<AtomicU64>,
}

impl SubscriptionGuard {
    /// The highest sequence number handed to the subscriber, which the subscription
    /// raises.
    pub(crate) fn reached(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.reached)
    }
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        let connections = Arc::clone(&self.connections);
        connections.unsubscribe(self);
    }
}

/// A `prompt.send` of one connection, counted until it is dropped.
#[derive(Debug)]
pub(crate) struct PromptGuard {
    connections: Arc<Connections>,
    conn_id: ConnId,
    /// False when the connection had no hello, so nothing was counted.
    counted: bool,
    sent_to: Option<ConversationId>,
}

impl PromptGuard {
    /// Records that the prompt went to `conversation`, which the connection then counts
    /// as one it may show until it closes.
    pub(crate) fn sent_to(&mut self, conversation: ConversationId) {
        self.sent_to = Some(conversation);
    }
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        if self.counted {
            self.connections.prompted(self.conn_id, self.sent_to);
        }
    }
}

#[cfg(test)]
mod tests;
