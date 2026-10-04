//! What the daemon keeps per connection: the hello's tty and client, the open
//! conversation subscriptions, and the lease of `lease.report`.
//!
//! The transport carries only `{surface, uid, pid, conn_id}` with each request; the rest
//! of the hello lives here from the accepted hello until `Dispatcher::closed`. The
//! notices use it to decide whether a terminal already shows a conversation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_protocol::{ConversationId, Origin, PtyId};
use efr_transport::ConnId;
use jiff::Timestamp;

/// How long a lease lasts without another report. Clients report about every 25
/// seconds, so two missed reports end it.
pub(crate) const LEASE_TTL: Duration = Duration::from_secs(60);

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
}

#[derive(Debug)]
struct Conn {
    hello: HelloInfo,
    /// Open `conversation.subscribe` requests per conversation.
    subscriptions: HashMap<ConversationId, usize>,
    lease: Option<Lease>,
}

/// Every connection with an accepted hello.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    conns: Mutex<HashMap<ConnId, Conn>>,
}

impl Connections {
    /// Records the hello of `conn_id`.
    pub(crate) fn opened(&self, conn_id: ConnId, hello: HelloInfo) {
        self.lock().insert(conn_id, Conn { hello, subscriptions: HashMap::new(), lease: None });
    }

    /// Forgets `conn_id` and its lease.
    pub(crate) fn closed(&self, conn_id: ConnId) {
        self.lock().remove(&conn_id);
    }

    /// The tty the hello of `conn_id` named.
    pub(crate) fn tty(&self, conn_id: ConnId) -> Option<String> {
        self.lock().get(&conn_id).and_then(|conn| conn.hello.tty.clone())
    }

    /// Records an open subscription of `conn_id` to `conversation` until the guard is
    /// dropped, which happens when the request ends however it ends.
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        conn_id: ConnId,
        conversation: ConversationId,
    ) -> SubscriptionGuard {
        if let Some(conn) = self.lock().get_mut(&conn_id) {
            *conn.subscriptions.entry(conversation).or_default() += 1;
        }
        SubscriptionGuard { connections: Arc::clone(self), conn_id, conversation }
    }

    /// Replaces the lease of `conn_id`.
    pub(crate) fn report_lease(&self, conn_id: ConnId, lease: Lease) {
        if let Some(conn) = self.lock().get_mut(&conn_id) {
            conn.lease = Some(lease);
        }
    }

    /// True when a client in `tty` shows `conversation` at `now`: a connection whose
    /// hello named the tty has a subscription to it, or a live lease that names it.
    pub(crate) fn attached(&self, tty: &str, conversation: ConversationId, now: Timestamp) -> bool {
        self.lock().values().filter(|conn| conn.hello.tty.as_deref() == Some(tty)).any(|conn| {
            conn.subscriptions.get(&conversation).is_some_and(|open| *open > 0)
                || conn.lease.as_ref().is_some_and(|lease| {
                    lease.expires_at > now && lease.conversations.contains(&conversation)
                })
        })
    }

    /// True when a live lease names `pty_id`.
    pub(crate) fn watches_pty(&self, pty_id: PtyId, now: Timestamp) -> bool {
        self.lock().values().any(|conn| {
            conn.lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now && lease.ptys.contains(&pty_id))
        })
    }

    fn unsubscribe(&self, conn_id: ConnId, conversation: ConversationId) {
        if let Some(conn) = self.lock().get_mut(&conn_id)
            && let Some(open) = conn.subscriptions.get_mut(&conversation)
        {
            *open = open.saturating_sub(1);
            if *open == 0 {
                conn.subscriptions.remove(&conversation);
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ConnId, Conn>> {
        // Every critical section leaves the map whole, so a poisoned lock is still good.
        self.conns.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An open subscription, counted until it is dropped.
#[derive(Debug)]
pub(crate) struct SubscriptionGuard {
    connections: Arc<Connections>,
    conn_id: ConnId,
    conversation: ConversationId,
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.connections.unsubscribe(self.conn_id, self.conversation);
    }
}

#[cfg(test)]
mod tests;
