//! The warm-up of a prompt's connection: the provider's connection is ready before the
//! prompt's request (`provider.warm` and the start of `prompt.send`).
//!
//! The zsh plugin sends `provider.warm` (through `efr warm`) once for each prompt line,
//! when the user types the `,` of a prompt, and in sticky mode at the first key of a
//! line, so the handshake (DNS, TCP, TLS and the WebSocket upgrade) and the refresh of a
//! token that ends soon happen while the user types. `prompt.send` starts the same
//! warm-up when it arrives, so a pasted prompt gains the few milliseconds of efrd's own
//! work before its request; it takes the prompt's model or the default, and does not
//! read the conversation's pick, so it adds no read of the store to the prompt's.
//!
//! efrd picks what the next prompt of the terminal would use ([`Target`]): the
//! terminal's conversation of the scope of `pwd` while it continues (not one that was
//! idle for `conversation.tty_idle_hours`), the model that the terminal asks for, else
//! the conversation's pick (the model of its newest turn when a prompt picked it, while
//! a list still has it), else the config's default, and the provider of that model as a
//! turn picks it. A wrong guess costs one warm-up and nothing else.
//!
//! The provider does the part that it owns ([`Provider::warm`]): it asks for its token,
//! which refreshes one that ends soon, and on the WebSocket path it opens the
//! conversation's socket or pings a quiet one. A call over the shared HTTP pool has no
//! way to open a connection without a request, so efrd then fetches the provider's model
//! list, a request that it sends anyway, which opens the pooled connection that the
//! prompt's request then takes. That fetch runs at most once for each provider in
//! [`HTTP_WARM_GAP`], and not within that time of a prompt of the provider, whose
//! connection is still in the pool. The start of `prompt.send` never fetches: its own
//! request follows at once.
//!
//! A warm-up never sends a model request, so it never costs a token, and it carries no
//! text of the prompt: only the terminal, the directory and the asked model reach efrd
//! before Enter. It runs in a task of its own, never holds a prompt and never fails one;
//! what goes wrong only shows in the debug log.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_protocol::{ConversationId, Event, RouteScope};
use efr_provider::{Provider, Warmup};
use efr_stdx::time::Clock;
use jiff::{SignedDuration, Timestamp};

use crate::catalog::Models;
use crate::methods::read_place;
use crate::routing::{self, Verdict};
use crate::state::State;

/// The shortest time between two fetches of a provider's model list for a warm-up, and
/// between a prompt of a provider and such a fetch.
pub(crate) const HTTP_WARM_GAP: Duration = Duration::from_secs(60);

/// How many of a conversation's newest events efrd reads to find its newest turn's
/// settings. A turn's start is rarely further back; when it is, the warm-up takes the
/// config's default.
const SETTINGS_PAGE: u32 = 200;

/// When each provider last had a prompt or an HTTP warm-up.
#[derive(Debug)]
pub(crate) struct Warmups {
    clock: Arc<dyn Clock>,
    last: Mutex<HashMap<String, Timestamp>>,
}

impl Warmups {
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Self {
        Warmups { clock, last: Mutex::new(HashMap::new()) }
    }

    /// A prompt goes to `provider` now: its request opens or takes the pooled
    /// connection.
    pub(crate) fn used(&self, provider: &str) {
        let now = self.clock.now();
        self.lock().insert(provider.to_owned(), now);
    }

    /// True, and noted, when an HTTP warm-up of `provider` may fetch now.
    fn http_due(&self, provider: &str) -> bool {
        let now = self.clock.now();
        let gap = SignedDuration::try_from(HTTP_WARM_GAP).unwrap_or_default();
        let mut last = self.lock();
        let recent = last.get(provider).is_some_and(|at| now.duration_since(*at) < gap);
        if !recent {
            last.insert(provider.to_owned(), now);
        }
        !recent
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Timestamp>> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a warm-up is for: the provider, its model list and the call that it prepares.
pub(crate) struct Target {
    provider: Arc<dyn Provider>,
    models: Option<Arc<Models>>,
    warmup: Warmup,
}

impl Target {
    /// The provider's id, such as `openai-subscription`.
    pub(crate) fn provider(&self) -> String {
        self.provider.id().as_str().to_owned()
    }
}

/// Where the warm-up of a prompt comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// The user types a prompt line (`provider.warm`): the HTTP pool may be warmed.
    Typing,
    /// `prompt.send` arrived: its request follows at once, so only the provider's own
    /// connection and the token are worth a start.
    Prompt,
}

/// What efrd knows of the next prompt of a terminal.
#[derive(Debug, Clone, Default)]
pub(crate) struct Next {
    pub(crate) tty: Option<String>,
    pub(crate) pwd: Option<PathBuf>,
    /// The model that the terminal asks for.
    pub(crate) model: Option<String>,
    /// The provider that the prompt names, such as an account.
    pub(crate) provider: Option<String>,
    pub(crate) new_conversation: bool,
    /// The conversation, when the prompt names it or routing picked it.
    pub(crate) conversation: Option<ConversationId>,
    /// Read the conversation's pick from the store. The start of `prompt.send` does
    /// not: that read would compete with the turn's own read of the history, and the
    /// default provider is the right one unless a prompt picked another.
    pub(crate) read_pick: bool,
}

/// The target of the warm-up of the prompt `next`, as its turn would pick it.
pub(crate) async fn target(state: &State, next: &Next) -> Target {
    let settings = Arc::clone(&state.settings.borrow());
    let conversation = match next.conversation {
        Some(conversation_id) => Some(conversation_id),
        None if next.new_conversation => None,
        None => continued(state, next).await,
    };
    let pick = match (&next.model, conversation) {
        (None, Some(conversation_id)) if next.read_pick => {
            newest_pick(state, conversation_id).await
        }
        _ => None,
    };
    let (model, named, picked) = match (&next.model, pick) {
        (Some(model), _) => (Some(model.clone()), next.provider.clone(), false),
        (None, Some((model, provider))) => (Some(model), provider, true),
        (None, None) => (None, None, false),
    };
    let (provider, model) =
        state.providers.warm_target(&settings, model.as_deref(), named.as_deref(), picked);
    let models = state.providers.models_of(provider.id().as_str());
    let key = conversation.map(|conversation_id| conversation_id.to_string());
    Target { provider, models, warmup: Warmup::new(model, key) }
}

/// Starts the warm-up of `target` in a task of its own.
pub(crate) fn start(warmups: Arc<Warmups>, target: Target, trigger: Trigger) {
    tokio::spawn(async move {
        efr_stdx::mark::hit("warm.begin");
        let Target { provider, models, warmup } = target;
        let id = provider.id().as_str().to_owned();
        let own = provider.warm(&warmup).await;
        let fetched = if !own
            && trigger == Trigger::Typing
            && let Some(models) = models.filter(|models| models.fetches())
            && warmups.http_due(&id)
        {
            models.warm().await;
            true
        } else {
            false
        };
        tracing::debug!(provider = %id, model = %warmup.model, conversation = ?warmup.conversation, own, fetched, "warmed the connection of the next prompt");
        efr_stdx::mark::hit("warm.done");
    });
}

/// The terminal's conversation of the scope of `next.pwd` while the next prompt would
/// continue it: one that its shell holds and that was not idle too long.
async fn continued(state: &State, next: &Next) -> Option<ConversationId> {
    let tty = next.tty.as_deref()?;
    let settings = Arc::clone(&state.settings.borrow());
    let (registry, resolved) = read_place(state, next.pwd.clone()).await;
    let pwd_key = next
        .pwd
        .as_deref()
        .map_or(RouteScope::Outside, |pwd| routing::route_key(&registry, pwd, resolved.as_deref()));
    let current = state.conversations.current(tty);
    let key = routing::slot_key(
        current.as_ref().map(|(key, _)| key),
        &pwd_key,
        settings.conversation.follow_project,
        next.pwd.is_some(),
        false,
    );
    let slot = state.conversations.slot(tty, &key)?;
    let conversation_id = slot.conversation_id;
    let summary = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await
        .ok()
        .flatten()?;
    let idle = routing::idle_limit(settings.conversation.tty_idle_hours);
    let alive = |pid: u32| state.procs.alive(pid);
    match routing::continues(
        slot.shell_pid,
        None,
        Some(summary.updated_at),
        state.clock.now(),
        idle,
        alive,
    ) {
        Verdict::Continues => Some(conversation_id),
        _ => None,
    }
}

/// The model and the provider that a prompt of the conversation picked for its newest
/// turn, when one did. Whether a list still has it, [`Providers::warm_target`] checks.
///
/// [`Providers::warm_target`]: crate::providers::Providers::warm_target
async fn newest_pick(
    state: &State,
    conversation_id: ConversationId,
) -> Option<(String, Option<String>)> {
    let page = state
        .readers
        .with(move |conn| {
            efr_store::events::read_conversation_before(conn, conversation_id, None, SETTINGS_PAGE)
        })
        .await
        .ok()?;
    let settings = page.iter().rev().find_map(|envelope| match &envelope.event {
        Event::TurnStarted { settings, .. } => Some(settings.clone()),
        _ => None,
    })??;
    settings.overridden.model.then_some((settings.model, settings.provider))
}

#[cfg(test)]
mod tests;
