//! The conversations the daemon runs: one actor each, started when a request needs it,
//! and the active conversation of every terminal.
//!
//! A terminal (`$TTY`) has one active conversation: the newest one started from it. A
//! `,` line without a conversation id goes there, and `,new` starts a new one. The map
//! mirrors the `tty` column of the conversations projection, which the store clears on
//! the older conversation when a newer one takes the terminal, and is loaded from it at
//! startup.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use efr_conversation::{
    ConversationActor, ConversationConfig, ConversationDeps, ConversationHandle, ConversationStart,
};
use efr_protocol::{ConversationId, Origin};
use efr_store::Readers;

use crate::DaemonError;

/// How many conversations one startup read asks for at a time.
const PAGE: u32 = 256;

/// The conversation actors and the terminals' active conversations.
#[derive(Debug)]
pub(crate) struct Conversations {
    config: ConversationConfig,
    deps: ConversationDeps,
    live: Mutex<HashMap<ConversationId, ConversationHandle>>,
    ttys: Mutex<HashMap<String, ConversationId>>,
}

impl Conversations {
    /// Starts actors with `config` and `deps`, with the terminals' active conversations
    /// from `ttys`.
    pub(crate) fn new(
        config: ConversationConfig,
        deps: ConversationDeps,
        ttys: HashMap<String, ConversationId>,
    ) -> Self {
        Conversations { config, deps, live: Mutex::default(), ttys: Mutex::new(ttys) }
    }

    /// The running actor of `conversation_id`, if one runs.
    pub(crate) fn live(&self, conversation_id: ConversationId) -> Option<ConversationHandle> {
        let mut live = self.lock_live();
        match live.get(&conversation_id) {
            Some(handle) if !handle.is_closed() => Some(handle.clone()),
            Some(_) => {
                live.remove(&conversation_id);
                None
            }
            None => None,
        }
    }

    /// The actor of an existing conversation, started when none runs. The caller has
    /// checked that the conversation exists.
    pub(crate) fn open(&self, conversation_id: ConversationId) -> ConversationHandle {
        let mut live = self.lock_live();
        if let Some(handle) = live.get(&conversation_id).filter(|handle| !handle.is_closed()) {
            return handle.clone();
        }
        let handle = ConversationActor::spawn(
            conversation_id,
            ConversationStart::Existing,
            self.config.clone(),
            self.deps.clone(),
        );
        live.insert(conversation_id, handle.clone());
        handle
    }

    /// The actor of a new conversation. Nothing is recorded until its first prompt
    /// commits; [`forget`](Self::forget) drops it when that prompt fails.
    pub(crate) fn create(
        &self,
        conversation_id: ConversationId,
        origin: Origin,
        tty: Option<String>,
    ) -> ConversationHandle {
        let handle = ConversationActor::spawn(
            conversation_id,
            ConversationStart::New { origin, tty },
            self.config.clone(),
            self.deps.clone(),
        );
        self.lock_live().insert(conversation_id, handle.clone());
        handle
    }

    /// Drops the actor of a new conversation whose first prompt was not recorded.
    pub(crate) async fn forget(&self, conversation_id: ConversationId) {
        let handle = self.lock_live().remove(&conversation_id);
        if let Some(handle) = handle {
            // An actor that already stopped has nothing left to stop.
            let _ = handle.shutdown().await;
        }
    }

    /// Makes `conversation_id` the active conversation of `tty`.
    pub(crate) fn activate(&self, tty: &str, conversation_id: ConversationId) {
        self.lock_ttys().insert(tty.to_owned(), conversation_id);
    }

    /// The active conversation of `tty`.
    pub(crate) fn active(&self, tty: &str) -> Option<ConversationId> {
        self.lock_ttys().get(tty).copied()
    }

    /// How many actors run.
    pub(crate) fn count(&self) -> usize {
        let mut live = self.lock_live();
        live.retain(|_, handle| !handle.is_closed());
        live.len()
    }

    /// Stops every actor and waits until each has stopped.
    pub(crate) async fn shutdown_all(&self) {
        let handles: Vec<ConversationHandle> = self.lock_live().drain().map(|(_, h)| h).collect();
        for handle in handles {
            // An actor that already stopped has nothing left to stop.
            let _ = handle.shutdown().await;
        }
    }

    fn lock_live(&self) -> MutexGuard<'_, HashMap<ConversationId, ConversationHandle>> {
        // Every critical section leaves the map whole, so a poisoned lock is still good.
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_ttys(&self) -> MutexGuard<'_, HashMap<String, ConversationId>> {
        self.ttys.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The active conversation of every terminal, from the conversations projection.
pub(crate) async fn load_ttys(
    readers: &Readers,
) -> Result<HashMap<String, ConversationId>, DaemonError> {
    let mut ttys = HashMap::new();
    let mut before = None;
    loop {
        let page =
            readers.with(move |conn| efr_store::conversations::list(conn, before, PAGE)).await?;
        let Some(last) = page.last() else {
            return Ok(ttys);
        };
        before = Some(last.last_seq);
        for summary in page {
            if let Some(tty) = summary.tty {
                // The projection keeps at most one conversation per terminal.
                ttys.entry(tty).or_insert(summary.id);
            }
        }
    }
}
