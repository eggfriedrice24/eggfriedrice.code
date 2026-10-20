//! The conversations the daemon runs: one actor each, started when a request needs it,
//! and the slots of every terminal.
//!
//! A terminal (`$TTY`) keeps one conversation per scope key (`efr_protocol::RouteScope`):
//! one for each registered project that it worked in, and one for every place outside
//! projects. Each such entry is a [`Slot`], with the pid of the shell that holds it and
//! the sequence number of its newest routed event; the terminal's current slot is the
//! one with the highest, the slot of its last prompt, `,new` or resume. A conversation
//! sits in at most one slot of all terminals. `methods/prompt_send.rs` decides where a
//! prompt goes (`routing.rs` holds the rules).
//!
//! The map mirrors the `tty`, `route`, `shell_pid` and `routed_seq` columns of the
//! conversations projection, and is loaded from them at startup in one query (S-18,
//! F3). Each change of the map follows one event that the same request recorded, and
//! the projection applies the same rule to it: `conversation_created` (a new slot),
//! `terminal_attached` (a slot moves) and `prompt_queued` from the slot's terminal (it
//! becomes current, and its first known pid stays). The map changes only after the
//! event committed; a claim for a pick-up that then fails is released. A slot whose
//! shell died stays until a new holder replaces it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_conversation::{
    ConfigSource, ConversationActor, ConversationDeps, ConversationHandle, ConversationStart,
};
use efr_protocol::{ConversationId, Origin, RouteScope, Seq};
use efr_store::Readers;
use efr_store::conversations::SlotRow;
use efr_transport::ConnectionContext;

use crate::DaemonError;
use crate::state::State;

/// One slot of a terminal: the conversation that it keeps for one scope key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) conversation_id: ConversationId,
    /// The pid of the shell that holds the slot, when it said. Per slot, not per
    /// terminal: a new shell on a reused `/dev/pts` must not make the dead shell's
    /// other slots look alive.
    pub(crate) shell_pid: Option<u32>,
    /// The sequence number of the slot's newest routed event; the terminal's current
    /// slot has the highest.
    pub(crate) routed_seq: Seq,
}

/// The slots of one terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Terminal {
    pub(crate) slots: HashMap<RouteScope, Slot>,
}

/// Where an open terminal holds a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    /// A slot of a terminal whose shell runs, or whose shell is not known.
    Slot { tty: String },
    /// A terminal follows it: an open `conversation.subscribe` from a connection whose
    /// hello named a tty (S-21). A phone has no tty, so it never holds one.
    Follow { tty: String },
}

impl Held {
    /// The terminal that holds it.
    pub(crate) fn tty(&self) -> &str {
        match self {
            Held::Slot { tty } | Held::Follow { tty } => tty,
        }
    }
}

/// The conversation actors and the terminals' slots.
#[derive(Debug)]
pub(crate) struct Conversations {
    config: Arc<dyn ConfigSource>,
    deps: ConversationDeps,
    live: Mutex<HashMap<ConversationId, ConversationHandle>>,
    ttys: Mutex<Routes>,
}

/// The slots of every terminal, and the slots whose conversation another terminal
/// took since the start, for the line of that terminal's next prompt.
#[derive(Debug, Default)]
pub(crate) struct Routes {
    terminals: HashMap<String, Terminal>,
    /// (tty, key) -> (the terminal that took it, its title). In memory only.
    moved: HashMap<(String, RouteScope), (String, Option<String>)>,
}

impl Routes {
    /// The slots of `rows`, as the projection holds them.
    pub(crate) fn from_rows(rows: Vec<SlotRow>) -> Self {
        let mut terminals: HashMap<String, Terminal> = HashMap::new();
        for row in rows {
            terminals.entry(row.tty).or_default().slots.insert(
                row.route,
                Slot {
                    conversation_id: row.conversation_id,
                    shell_pid: row.shell_pid,
                    routed_seq: row.routed_seq,
                },
            );
        }
        Routes { terminals, moved: HashMap::new() }
    }

    /// The slot of `tty` for the key `route`.
    pub(crate) fn slot(&self, tty: &str, route: &RouteScope) -> Option<Slot> {
        self.terminals.get(tty).and_then(|terminal| terminal.slots.get(route)).copied()
    }

    /// The current slot of `tty`: the one with the newest routed event.
    pub(crate) fn current(&self, tty: &str) -> Option<(RouteScope, Slot)> {
        self.terminals
            .get(tty)?
            .slots
            .iter()
            .max_by_key(|(_, slot)| slot.routed_seq)
            .map(|(route, slot)| (route.clone(), *slot))
    }

    /// Every slot that holds `conversation_id`, with its terminal and key.
    pub(crate) fn holders(
        &self,
        conversation_id: ConversationId,
    ) -> Vec<(String, RouteScope, Slot)> {
        let mut found: Vec<_> = self
            .terminals
            .iter()
            .flat_map(|(tty, terminal)| {
                terminal
                    .slots
                    .iter()
                    .filter(|(_, slot)| slot.conversation_id == conversation_id)
                    .map(|(route, slot)| (tty.clone(), route.clone(), *slot))
            })
            .collect();
        found.sort_by_key(|holder| std::cmp::Reverse(holder.2.routed_seq));
        found
    }

    /// Makes `conversation_id`, a new conversation whose first prompt just committed,
    /// the conversation of `tty`'s slot `route`, held by `shell_pid`, with its newest
    /// routed event `routed_seq`.
    pub(crate) fn set_slot(
        &mut self,
        tty: &str,
        route: RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        routed_seq: Seq,
    ) {
        self.moved.remove(&(tty.to_owned(), route.clone()));
        self.terminals
            .entry(tty.to_owned())
            .or_default()
            .slots
            .insert(route, Slot { conversation_id, shell_pid, routed_seq });
    }

    /// Records a committed prompt `routed_seq` from `tty`'s shell `shell_pid` to
    /// `conversation_id`: when a slot of `tty` holds it, that slot becomes the current
    /// one, and it keeps the first pid that it learns, so a nested shell never takes it
    /// from the outer one. A prompt from another terminal changes nothing.
    pub(crate) fn routed(
        &mut self,
        tty: &str,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        routed_seq: Seq,
    ) {
        let Some(terminal) = self.terminals.get_mut(tty) else {
            return;
        };
        for slot in terminal.slots.values_mut() {
            if slot.conversation_id == conversation_id {
                slot.routed_seq = slot.routed_seq.max(routed_seq);
                if slot.shell_pid.is_none() {
                    slot.shell_pid = shell_pid;
                }
            }
        }
    }

    /// Claims `conversation_id` for `tty`'s slot `route` before its prompt is sent
    /// (F5): true when the slot still holds `expected` and no other slot whose shell
    /// runs (`alive`), or whose shell is not known, holds the conversation. Then the
    /// slot holds it with the shell `shell_pid` until [`attached`](Self::attached) or
    /// [`release_claim`](Self::release_claim). Of two terminals that claim the same
    /// conversation, the second fails, because the first one's shell runs.
    pub(crate) fn claim(
        &mut self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        expected: Option<ConversationId>,
        alive: &dyn Fn(u32) -> bool,
    ) -> bool {
        let target = self.slot(tty, route);
        if target.map(|slot| slot.conversation_id) != expected {
            return false;
        }
        let held_elsewhere = self.holders(conversation_id).into_iter().any(|(other, key, slot)| {
            (other != tty || &key != route) && slot.shell_pid.is_none_or(alive)
        });
        if held_elsewhere {
            return false;
        }
        let routed_seq = target.map_or(Seq::ZERO, |slot| slot.routed_seq);
        self.terminals
            .entry(tty.to_owned())
            .or_default()
            .slots
            .insert(route.clone(), Slot { conversation_id, shell_pid, routed_seq });
        true
    }

    /// Claims `conversation_id` for `tty`'s slot `route` for a resume that moves it
    /// from another terminal on the user's word (`take`): as [`claim`](Self::claim),
    /// but another slot that holds the conversation does not stop it. That slot lets
    /// it go at [`attached`](Self::attached).
    pub(crate) fn take(
        &mut self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        expected: Option<ConversationId>,
    ) -> bool {
        let target = self.slot(tty, route);
        if target.map(|slot| slot.conversation_id) != expected {
            return false;
        }
        let routed_seq = target.map_or(Seq::ZERO, |slot| slot.routed_seq);
        self.terminals
            .entry(tty.to_owned())
            .or_default()
            .slots
            .insert(route.clone(), Slot { conversation_id, shell_pid, routed_seq });
        true
    }

    /// Gives `tty`'s slot `route` back what it held `before` a claim whose prompt
    /// failed: nothing was recorded, so the map stays equal to the projection (S-18).
    pub(crate) fn release_claim(&mut self, tty: &str, route: &RouteScope, before: Option<Slot>) {
        let Some(terminal) = self.terminals.get_mut(tty) else {
            return;
        };
        match before {
            Some(slot) => {
                terminal.slots.insert(route.clone(), slot);
            }
            None => {
                terminal.slots.remove(route);
            }
        }
        if terminal.slots.is_empty() {
            self.terminals.remove(tty);
        }
    }

    /// Completes a claim whose `terminal_attached` and prompt committed, with the
    /// newest of them `routed_seq`: every other slot that held the conversation lets it
    /// go, as the projection's rule does, and remembers where it went, with its
    /// `title`, for the line of that terminal's next prompt.
    pub(crate) fn attached(
        &mut self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        routed_seq: Seq,
        title: Option<String>,
    ) {
        let mut left = Vec::new();
        for (other, terminal) in &mut self.terminals {
            terminal.slots.retain(|key, slot| {
                let keep =
                    slot.conversation_id != conversation_id || (other == tty && key == route);
                if !keep {
                    left.push((other.clone(), key.clone()));
                }
                keep
            });
        }
        self.terminals.retain(|_, terminal| !terminal.slots.is_empty());
        for key in left {
            self.moved.insert(key, (tty.to_owned(), title.clone()));
        }
        if let Some(slot) =
            self.terminals.get_mut(tty).and_then(|terminal| terminal.slots.get_mut(route))
        {
            slot.routed_seq = slot.routed_seq.max(routed_seq);
        }
    }

    /// Where the conversation of `tty`'s slot `route` went, when another terminal took
    /// it since the start; the record goes with the answer.
    pub(crate) fn take_moved(
        &mut self,
        tty: &str,
        route: &RouteScope,
    ) -> Option<(String, Option<String>)> {
        self.moved.remove(&(tty.to_owned(), route.clone()))
    }
}

impl Conversations {
    /// Starts actors that read their settings from `config`, with `deps`, and the
    /// terminals' slots from `slots`.
    pub(crate) fn new(
        config: Arc<dyn ConfigSource>,
        deps: ConversationDeps,
        slots: Vec<SlotRow>,
    ) -> Self {
        Conversations {
            config,
            deps,
            live: Mutex::default(),
            ttys: Mutex::new(Routes::from_rows(slots)),
        }
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
            Arc::clone(&self.config),
            self.deps.clone(),
        );
        live.insert(conversation_id, handle.clone());
        handle
    }

    /// The actor of a new conversation, the conversation of `tty`'s slot `route` when
    /// both are given. Nothing is recorded until its first prompt commits;
    /// [`forget`](Self::forget) drops it when that prompt fails.
    pub(crate) fn create(
        &self,
        conversation_id: ConversationId,
        origin: Origin,
        tty: Option<String>,
        route: Option<RouteScope>,
    ) -> ConversationHandle {
        let handle = ConversationActor::spawn(
            conversation_id,
            ConversationStart::New { origin, tty, route },
            Arc::clone(&self.config),
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

    /// The slot of `tty` for the key `route`.
    pub(crate) fn slot(&self, tty: &str, route: &RouteScope) -> Option<Slot> {
        self.lock_ttys().slot(tty, route)
    }

    /// The current slot of `tty`: the one with the newest routed event.
    pub(crate) fn current(&self, tty: &str) -> Option<(RouteScope, Slot)> {
        self.lock_ttys().current(tty)
    }

    /// Every slot that holds `conversation_id`, with its terminal and key: one, or two
    /// while a claim waits for its prompt.
    pub(crate) fn holders(
        &self,
        conversation_id: ConversationId,
    ) -> Vec<(String, RouteScope, Slot)> {
        self.lock_ttys().holders(conversation_id)
    }

    /// Every slot of every terminal, for a comparison with the projection.
    #[cfg(test)]
    pub(crate) fn terminals(&self) -> HashMap<String, Terminal> {
        self.lock_ttys().terminals.clone()
    }

    /// See [`Routes::set_slot`].
    pub(crate) fn set_slot(
        &self,
        tty: &str,
        route: RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        routed_seq: Seq,
    ) {
        self.lock_ttys().set_slot(tty, route, conversation_id, shell_pid, routed_seq);
    }

    /// See [`Routes::routed`].
    pub(crate) fn routed(
        &self,
        tty: &str,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        routed_seq: Seq,
    ) {
        self.lock_ttys().routed(tty, conversation_id, shell_pid, routed_seq);
    }

    /// See [`Routes::claim`]; the checks and the change are one lock section (F5).
    pub(crate) fn claim(
        &self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        expected: Option<ConversationId>,
        alive: &dyn Fn(u32) -> bool,
    ) -> bool {
        self.lock_ttys().claim(tty, route, conversation_id, shell_pid, expected, alive)
    }

    /// See [`Routes::take`]; one lock section, as a claim.
    pub(crate) fn take(
        &self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        shell_pid: Option<u32>,
        expected: Option<ConversationId>,
    ) -> bool {
        self.lock_ttys().take(tty, route, conversation_id, shell_pid, expected)
    }

    /// See [`Routes::release_claim`].
    pub(crate) fn release_claim(&self, tty: &str, route: &RouteScope, before: Option<Slot>) {
        self.lock_ttys().release_claim(tty, route, before);
    }

    /// See [`Routes::attached`].
    pub(crate) fn attached(
        &self,
        tty: &str,
        route: &RouteScope,
        conversation_id: ConversationId,
        routed_seq: Seq,
        title: Option<String>,
    ) {
        self.lock_ttys().attached(tty, route, conversation_id, routed_seq, title);
    }

    /// See [`Routes::take_moved`].
    pub(crate) fn take_moved(
        &self,
        tty: &str,
        route: &RouteScope,
    ) -> Option<(String, Option<String>)> {
        self.lock_ttys().take_moved(tty, route)
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

    fn lock_ttys(&self) -> MutexGuard<'_, Routes> {
        self.ttys.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The slots of every terminal, from the conversations projection.
pub(crate) async fn load_slots(readers: &Readers) -> Result<Vec<SlotRow>, DaemonError> {
    Ok(readers.with(efr_store::conversations::slots).await?)
}

/// Where an open terminal other than `asking` holds `conversation_id`: a slot whose
/// shell runs (a zombie counts as gone) or whose shell is not known, or an open follow
/// from a connection whose hello named a tty. A phone that follows it never holds it.
pub(crate) fn held(
    state: &State,
    conversation_id: ConversationId,
    asking: Option<&str>,
) -> Option<Held> {
    if let Some((tty, _, _)) =
        state.conversations.holders(conversation_id).into_iter().find(|(tty, _, slot)| {
            Some(tty.as_str()) != asking && slot.shell_pid.is_none_or(|pid| state.procs.alive(pid))
        })
    {
        return Some(Held::Slot { tty });
    }
    state
        .connections
        .followed_from(conversation_id)
        .into_iter()
        .find(|tty| Some(tty.as_str()) != asking)
        .map(|tty| Held::Follow { tty })
}

/// True when the peer of `context` runs on the terminal `tty` (S-12): the connection
/// is the shell plugin's, or `efr`'s with a hello tty, never a phone's or a proxy's,
/// and the controlling terminal of the peer's process is the device of `tty`. When
/// either cannot be read, false.
pub(crate) fn peer_on_tty(state: &State, context: &ConnectionContext, tty: &str) -> bool {
    let surface = match context.surface() {
        Origin::Shell => true,
        Origin::Cli => state.connections.tty(context.conn_id()).is_some(),
        _ => false,
    };
    if !surface {
        return false;
    }
    let Some(pid) = context.pid() else {
        return false;
    };
    match (state.procs.tty_of(pid), state.procs.device_of(tty)) {
        (Some(peer), Some(device)) => peer == device,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
