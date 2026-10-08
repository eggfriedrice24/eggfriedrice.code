//! What the daemon hands a conversation: settings through a [`ConfigSource`],
//! collaborators by handle.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_permissions::{Engine, Policy};
use efr_protocol::{Mode, ModelInfo, Origin, SandboxStatus};
use efr_provider::Provider;
use efr_scope::Home;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use efr_store::{Readers, WriterHandle};
use jiff::tz::TimeZone;
use serde_json::{Map, Value};
use tokio::sync::{broadcast, watch};

use crate::{
    CompactionConfig, ConversationDraft, ExitJudge, HistoryLimits, ScopeResolver, Toolbox,
};

/// The settings of one conversation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConversationConfig {
    /// The default model of a turn, the provider's model id. A prompt may name another
    /// one from [`models`](Self::models).
    pub model: String,
    /// The default reasoning effort of a turn; `None` leaves it to the backend. A
    /// prompt may name another one that its model takes.
    pub effort: Option<String>,
    /// The default permission mode of a turn. A prompt may name another one; a turn
    /// from a remote origin runs with at most `cautious`. The mode picks the built-in
    /// policy that the user's rules follow.
    pub mode: Mode,
    /// The effective model list: the models a turn may use, each with the efforts it
    /// takes. Empty when the provider does not say, and then any model id is passed
    /// through and the backend's answer decides.
    pub models: Vec<ModelInfo>,
    /// The static rules, sent as the request's system prompt. The live state is not
    /// here; it rides on the newest prompt.
    pub system_prompt: Option<String>,
    /// The most tokens one model call may produce; the provider's default when absent.
    pub max_output_tokens: Option<u32>,
    /// Options only one provider understands, passed through on every request.
    pub provider_options: Map<String, Value>,
    /// The directory that holds every conversation's `$SCRATCH`,
    /// `$XDG_DATA_HOME/efr/scratch`.
    pub scratch_root: PathBuf,
    /// The time zone of the date in a scratch directory's name.
    pub time_zone: TimeZone,
    /// The machine facts for the preamble.
    pub host: HostInfo,
    /// The conversation's own permission rules, read after the machine policy.
    pub policy: Policy,
    /// How much history a request carries.
    pub history: HistoryLimits,
    /// When the context is compacted on its own (`[compaction]`). Read when a turn
    /// starts.
    pub compaction: CompactionConfig,
    /// The shortest time between two `assistant_message_updated` events, and between
    /// two `tool_call_output_updated` events of one call.
    pub update_interval: Duration,
    /// The shortest time between two drafts of a turn ([`ConversationDraft`]): the text,
    /// the reasoning and the tool input as they arrive from the model, for live clients
    /// only. Zero sends a draft for every change. Read when the turn starts.
    pub draft_interval: Duration,
    /// How long a parked approval waits before it expires; `None` waits until the user
    /// answers or interrupts.
    pub approval_timeout: Option<Duration>,
    /// The most model calls in one turn; a turn that needs more fails.
    pub max_model_calls: u32,
    /// The most prompts that may wait behind the running turn.
    pub max_queued: usize,
}

impl ConversationConfig {
    /// Settings for `model` with scratch directories under `scratch_root`, and the
    /// defaults for the rest: the backend's default effort, the `cautious` mode, no
    /// model list (any model), no system prompt, UTC dates, no machine facts, no
    /// conversation rules, [`HistoryLimits::default`], [`CompactionConfig::default`],
    /// 200 ms between updates, 16 ms
    /// between drafts, no approval timeout, 64 model calls per turn and 16 queued prompts.
    pub fn new(model: impl Into<String>, scratch_root: impl Into<PathBuf>) -> Self {
        ConversationConfig {
            model: model.into(),
            effort: None,
            mode: Mode::default(),
            models: Vec::new(),
            system_prompt: None,
            max_output_tokens: None,
            provider_options: Map::new(),
            scratch_root: scratch_root.into(),
            time_zone: TimeZone::UTC,
            host: HostInfo::default(),
            policy: Policy::empty(),
            history: HistoryLimits::default(),
            compaction: CompactionConfig::default(),
            update_interval: Duration::from_millis(200),
            draft_interval: Duration::from_millis(16),
            approval_timeout: None,
            max_model_calls: 64,
            max_queued: 16,
        }
    }

    /// Sets the system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    /// Sets the time zone of scratch directory names.
    #[must_use]
    pub fn with_time_zone(mut self, time_zone: TimeZone) -> Self {
        self.time_zone = time_zone;
        self
    }

    /// Sets the machine facts.
    #[must_use]
    pub fn with_host(mut self, host: HostInfo) -> Self {
        self.host = host;
        self
    }
}

/// Where a conversation reads its settings: the latest value each time a unit of work
/// starts.
///
/// A turn reads the settings once, when it starts, and keeps them until it ends, so a
/// change never reaches a running turn's model, prompt or limits. Each tool call reads
/// the approval timeout and the update interval when it starts. A prompt reads them
/// when it arrives (the queue limit), and the actor when a turn ends (how many turns it
/// keeps for the history).
/// The daemon implements it over its settings watch; a `watch::Receiver` of the
/// settings is one too.
pub trait ConfigSource: Send + Sync + fmt::Debug {
    /// The latest settings.
    fn current(&self) -> Arc<ConversationConfig>;
}

impl ConfigSource for watch::Receiver<Arc<ConversationConfig>> {
    fn current(&self) -> Arc<ConversationConfig> {
        Arc::clone(&self.borrow())
    }
}

/// Machine facts for the preamble, gathered once by the daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostInfo {
    /// The host name, used when the prompt's shell context names none.
    pub hostname: Option<String>,
    /// The operating system and distribution, such as `Arch Linux` from
    /// `/etc/os-release`.
    pub os: Option<String>,
}

impl HostInfo {
    /// Facts with the given host name and operating system.
    pub fn new(hostname: Option<String>, os: Option<String>) -> Self {
        HostInfo { hostname, os }
    }
}

/// The collaborators of one conversation.
#[derive(Debug, Clone)]
pub struct ConversationDeps {
    /// The model.
    pub provider: Arc<dyn Provider>,
    /// The tools.
    pub toolbox: Arc<dyn Toolbox>,
    /// The permission engine. The daemon sends a new one when the policy or the project
    /// registry changes; each tool call reads the latest.
    pub engine: watch::Receiver<Arc<Engine>>,
    /// The scope of each turn.
    pub scope: Arc<dyn ScopeResolver>,
    /// The store's writer: every event goes through it.
    pub writer: WriterHandle,
    /// The store's readers, for history.
    pub readers: Readers,
    /// Time, for update coalescing, approval timeouts and ids.
    pub clock: Arc<dyn Clock>,
    /// Randomness, for ids.
    pub rng: Arc<dyn Rng>,
    /// The user's home directory.
    pub home: Home,
    /// The latest result of the sandbox probe, read when a prompt arrives and when a
    /// turn starts: an `auto` turn without an available sandbox runs as `cautious`,
    /// with the probe's reason.
    pub sandbox: watch::Receiver<SandboxStatus>,
    /// Who judges an exit before the user (the classifier, from phase 3); `None` asks
    /// the user about every exit, as phase 1 does.
    pub judge: Option<Arc<dyn ExitJudge>>,
    /// Where the turns send their drafts, for the subscribers that asked for them. The
    /// daemon holds the other end; nothing stores a draft. A send without a receiver
    /// costs nothing, and a turn does no draft work while nobody listens.
    pub drafts: broadcast::Sender<ConversationDraft>,
}

/// How a conversation's actor starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationStart {
    /// The conversation is new: `conversation_created` is recorded with its first
    /// prompt, so a conversation never exists without one.
    New {
        /// The surface that started it.
        origin: Origin,
        /// The terminal it becomes the active conversation of.
        tty: Option<String>,
    },
    /// The conversation exists in the log, as after a restart or when the daemon
    /// started its actor again after an idle stop.
    Existing,
}
