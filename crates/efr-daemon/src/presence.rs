//! Presence per conversation: whether a person is there to answer its questions (efr's
//! auto spec, section 9.5).
//!
//! A conversation is present while an attached client had input in it within
//! `presence.away_after_minutes`: a prompt, a steer, an interrupt, an answer, keys for
//! the hidden shell (`methods.rs` marks each one). It is away after that, or when the
//! user says so with `presence.set` (`efr away`, `,away`), until the next input.
//!
//! No timer runs per conversation. The idle state is found when efrd first looks: when
//! a waiting question reads the presence (when it parks, then every 15 seconds), or
//! when the next input comes. Only
//! then is the change recorded as `presence_changed` with the source `timeout`, so the
//! event can come later than the moment the limit passed. A change by input or by the
//! user is recorded at once. A conversation that the table does not know yet, such as
//! the first one after a restart, counts as present from the moment efrd first sees it.
//!
//! The push of an away question to the user's phones does nothing: no phone client
//! exists yet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::PresenceReader;
use efr_protocol::{ConversationId, Event, Presence, PresenceSource};
use efr_stdx::time::Clock;
use efr_store::{Batch, WriterHandle};
use jiff::Timestamp;
use tokio::sync::watch;

/// One change of presence to record: the conversation and its `presence_changed`.
pub(crate) type Change = (ConversationId, Event);

/// The presence of one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// The newest input of a person, or when efrd first saw the conversation.
    last_input: Timestamp,
    /// True after the user said that they are away, until the next input.
    set_away: bool,
    /// The presence that the log holds now: the newest `presence_changed`, or present
    /// when there is none.
    recorded: Presence,
}

impl Entry {
    fn new(now: Timestamp) -> Self {
        Entry { last_input: now, set_away: false, recorded: Presence::Present }
    }

    /// The presence at `now`, when no input for `away_after` counts as away.
    fn presence(&self, now: Timestamp, away_after: Duration) -> Presence {
        let idle = Duration::try_from(now.duration_since(self.last_input)).unwrap_or_default();
        if self.set_away || idle >= away_after { Presence::Away } else { Presence::Present }
    }

    /// Holds `presence` as recorded, and returns the event that records it in
    /// `conversation_id`, when the log holds another presence.
    fn record(
        &mut self,
        conversation_id: ConversationId,
        presence: Presence,
        source: PresenceSource,
    ) -> Option<Change> {
        if self.recorded == presence {
            return None;
        }
        self.recorded = presence;
        Some((conversation_id, Event::PresenceChanged { conversation_id, presence, source }))
    }
}

/// The presence of every conversation that had input since the daemon started.
#[derive(Debug)]
pub(crate) struct PresenceTable {
    entries: Mutex<HashMap<ConversationId, Entry>>,
    /// The daemon's settings; each look reads `presence.away_after_minutes` from the
    /// latest, so a change reaches the next one.
    settings: watch::Receiver<Arc<Settings>>,
    clock: Arc<dyn Clock>,
    writer: WriterHandle,
}

impl PresenceTable {
    /// An empty table that reads its limit from `settings`, its time from `clock`, and
    /// records its changes through `writer`.
    pub(crate) fn new(
        settings: watch::Receiver<Arc<Settings>>,
        clock: Arc<dyn Clock>,
        writer: WriterHandle,
    ) -> Self {
        PresenceTable { entries: Mutex::default(), settings, clock, writer }
    }

    /// Input of a person in `conversation_id` now: an idle state that the log does not
    /// hold yet is recorded first, then the return to present.
    pub(crate) async fn input(&self, conversation_id: ConversationId) {
        let changes = self.mark(conversation_id, true);
        self.record(changes).await;
    }

    /// As [`input`](Self::input), but only for a conversation that the table knows, so
    /// an id that names no conversation never enters it, and without the record: the
    /// caller [`record`](Self::record)s the changes later, such as after a prompt was
    /// routed, because each event moves the conversation's time of last activity.
    pub(crate) fn touch(&self, conversation_id: ConversationId) -> Vec<Change> {
        self.mark(conversation_id, false)
    }

    /// Records `changes` in one batch. A failure is logged: the table keeps its state,
    /// and the turn reads it either way.
    pub(crate) async fn record(&self, changes: Vec<Change>) {
        if let Some(batch) = batch(changes)
            && let Err(error) = self.writer.append(batch).await
        {
            tracing::warn!(error = %error, "a change of presence could not be recorded");
        }
    }

    /// Sets the presence of each of `conversations` as the user asked, and returns the
    /// changes to record, at most one per conversation. Present counts as input.
    pub(crate) fn set(&self, conversations: &[ConversationId], presence: Presence) -> Vec<Change> {
        let now = self.clock.now();
        let mut entries = self.lock();
        let mut changes = Vec::new();
        for &conversation_id in conversations {
            let entry = entries.entry(conversation_id).or_insert_with(|| Entry::new(now));
            if presence == Presence::Away {
                entry.set_away = true;
            } else {
                entry.set_away = false;
                entry.last_input = now;
            }
            changes.extend(entry.record(conversation_id, presence, PresenceSource::User));
        }
        changes
    }

    /// The conversations that the table knows, in no fixed order.
    pub(crate) fn known(&self) -> Vec<ConversationId> {
        self.lock().keys().copied().collect()
    }

    /// The conversations that the table knows and that are present now: those with
    /// input within `presence.away_after_minutes` and not set away.
    pub(crate) fn present(&self) -> Vec<ConversationId> {
        let (now, away_after) = (self.clock.now(), self.away_after());
        self.lock()
            .iter()
            .filter(|(_, entry)| entry.presence(now, away_after) == Presence::Present)
            .map(|(conversation_id, _)| *conversation_id)
            .collect()
    }

    /// Marks input in `conversation_id`, and adds it when `add` says so; the changes to
    /// record.
    fn mark(&self, conversation_id: ConversationId, add: bool) -> Vec<Change> {
        let (now, away_after) = (self.clock.now(), self.away_after());
        let mut entries = self.lock();
        let entry = match entries.get_mut(&conversation_id) {
            Some(entry) => entry,
            None if add => entries.entry(conversation_id).or_insert_with(|| Entry::new(now)),
            None => return Vec::new(),
        };
        let mut changes = Vec::new();
        if entry.presence(now, away_after) == Presence::Away {
            let source =
                if entry.set_away { PresenceSource::User } else { PresenceSource::Timeout };
            changes.extend(entry.record(conversation_id, Presence::Away, source));
        }
        entry.set_away = false;
        entry.last_input = now;
        changes.extend(entry.record(conversation_id, Presence::Present, PresenceSource::Input));
        changes
    }

    /// `presence.away_after_minutes` of the latest settings.
    fn away_after(&self) -> Duration {
        let minutes = self.settings.borrow().presence.away_after_minutes;
        Duration::from_secs(u64::from(minutes).saturating_mul(60))
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ConversationId, Entry>> {
        // Every critical section leaves the map whole, so a poisoned lock is still good.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One batch of `changes`, or `None` when there are none.
pub(crate) fn batch(changes: Vec<Change>) -> Option<Batch> {
    if changes.is_empty() {
        return None;
    }
    let batch = changes
        .into_iter()
        .fold(Batch::new(), |batch, (conversation_id, event)| batch.event(conversation_id, event));
    Some(batch)
}

impl PresenceReader for PresenceTable {
    fn presence(&self, conversation_id: ConversationId) -> Presence {
        let (now, away_after) = (self.clock.now(), self.away_after());
        let (presence, change) = {
            let mut entries = self.lock();
            let entry = entries.entry(conversation_id).or_insert_with(|| Entry::new(now));
            let presence = entry.presence(now, away_after);
            (presence, entry.record(conversation_id, presence, PresenceSource::Timeout))
        };
        // NOTE: the reader is sync, so the change is recorded in a task of its own. A
        // caller outside a runtime (none in efrd) only loses the event.
        if let Some(batch) = batch(change.into_iter().collect()) {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    let writer = self.writer.clone();
                    runtime.spawn(async move {
                        if let Err(error) = writer.append(batch).await {
                            tracing::warn!(error = %error, "a change of presence could not be recorded");
                        }
                    });
                }
                Err(_) => tracing::warn!("a change of presence was found outside a runtime"),
            }
        }
        presence
    }
}

#[cfg(test)]
mod tests;
