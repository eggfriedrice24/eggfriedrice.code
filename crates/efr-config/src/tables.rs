//! The tables of `config.toml` and their defaults.
//!
//! Each struct is one table of the file, read with `deny_unknown_fields`, so a typo is
//! an error and never silently does nothing. A key that the file leaves out has the
//! value of the table's `Default`. The doc comment of each field is its description in
//! the JSON schema.

use std::path::PathBuf;

use efr_permissions::Policy;
use efr_protocol::Mode;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

pub(crate) mod anthropic;
pub(crate) mod compaction;
pub(crate) mod render;
pub(crate) mod sandbox;
pub(crate) mod snapshot;

/// The tracing filter when nothing sets one: lifecycle lines only, as a service.
pub const DEFAULT_LOG: &str = "info";

/// The provider of new conversations when nothing names one.
pub const DEFAULT_PROVIDER: &str = "openai-subscription";

/// The providers the daemon knows how to build.
pub const PROVIDERS: &[&str] = &["openai-subscription", "openai-api", "anthropic-api"];

/// The `originator` of the subscription login and its requests when nothing names one.
pub const DEFAULT_ORIGINATOR: &str = "efr";

/// The static rules every request carries as its system prompt. The live state of the
/// user's shell is not here; the conversation adds it to each prompt.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are efr, a coding and system assistant that lives in the user's terminal. \
The user talks to you from their shell with lines that start with a comma. \
You run commands in a hidden zsh of your own, which starts in the user's working \
directory, and you read and write files with your tools. \
Each prompt starts with a <live_state> block: the state of the user's shell when they \
sent that prompt. Only the newest block is current; an older one shows the state at \
its own prompt. \
The user does not see your tools' output. For each tool call they see one dim line, \
such as shell: <command>, the last line of its output while it runs, and any approval \
question or input prompt. Say in your reply what the output showed that matters to \
them, and never refer to output as \"above\" or \"shown\". \
Every tool call is checked against the user's permission policy and the turn's \
permission mode, which the live state names: writes outside $SCRATCH may need the \
user's approval, and secrets are never readable. In the cautious mode, read-only \
commands such as ls, cat, rg, git status or systemctl status run at once when every \
argument is written out literally: no $VAR, no $(...), no redirection to a file, no \
pattern at the start of a word; any other command waits for the user's approval. \
To change a file, use the edit tool that you have: apply_patch or edit. Do not use \
sed -i or perl -pi, and do not write a whole file again with write_file for a small \
change; use write_file only for a new file or a full rewrite. A delete or a move of a \
file always waits for the user's approval. \
Prefer small, reversible steps, say what you change, and keep answers short. \
Put throwaway files in $SCRATCH.";

/// Which screen backend the hidden shells get.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ScreenChoice {
    /// ghostty when this build has it, otherwise vt100.
    #[default]
    Auto,
    /// The Zig-free vt100 backend.
    Vt100,
    /// The libghostty-vt backend.
    Ghostty,
}

impl ScreenChoice {
    /// Every choice, in the order the docs list them.
    pub const ALL: [ScreenChoice; 3] =
        [ScreenChoice::Auto, ScreenChoice::Vt100, ScreenChoice::Ghostty];

    /// The name in the file, in `EFR_SCREEN` and in `--screen`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScreenChoice::Auto => "auto",
            ScreenChoice::Vt100 => "vt100",
            ScreenChoice::Ghostty => "ghostty",
        }
    }
}

/// Whether model calls go over the Responses WebSocket transport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum WebSocketChoice {
    /// Follow the model: a model that prefers WebSockets uses one, any other model
    /// uses HTTP.
    #[default]
    Auto,
    /// Every model uses a WebSocket.
    On,
    /// Every model uses HTTP.
    Off,
}

/// What a hidden shell does with sudo's cached credentials after a call that ran sudo
/// or doas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SudoCache {
    /// sudo's own cache stays, for as long as sudoers keeps it (5 minutes by default).
    #[default]
    Keep,
    /// The hidden shell forgets the credentials after each call (`sudo -k`, `doas -L`),
    /// so the next sudo asks for the password again.
    PerCall,
}

/// `[model]`: the provider, the default model of a turn and what each request carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelSettings {
    /// The provider of new conversations: `openai-subscription` (the ChatGPT plan),
    /// `openai-api` (an OpenAI API key) or `anthropic-api` (an Anthropic API key, which
    /// this version of efrd cannot use yet). Needs a restart.
    pub provider: String,
    /// The default model of a turn, such as `gpt-6.1-sol`. Unset: the model with the
    /// best priority in the provider's model catalog (today the table built into efr).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The default reasoning effort, such as `low`, `medium` or `high`. Unset: the
    /// backend's own default for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The system prompt of every request.
    pub system_prompt: String,
    /// The most tokens one model call may produce. Unset: the provider's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        ModelSettings {
            provider: DEFAULT_PROVIDER.to_owned(),
            name: None,
            effort: None,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
            max_output_tokens: None,
        }
    }
}

/// `[openai]`: the OpenAI providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct OpenAiSettings {
    /// The `originator` of the subscription login and of every subscription request.
    /// Needs a restart.
    pub originator: String,
    /// Models added to the provider's model list, such as a new model before the
    /// backend lists it. A prompt may then name them; their efforts are not checked. An
    /// entry is a model id, or a table `{ id, context_window, max_output_tokens }` that
    /// also gives the model's limits in tokens. A table may name a model of the catalog
    /// to change its limits: its window up to the largest window that the catalog gives
    /// for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelEntry>>,
    /// Replaces the subscription backend's base URL. Needs a restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_base_url: Option<String>,
    /// Replaces the public API's base URL. Needs a restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    /// How model calls reach OpenAI: `auto` uses a WebSocket for each model that the
    /// model catalog marks with `prefer_websockets` and HTTP for the other models, `on`
    /// uses a WebSocket for every model, `off` uses HTTP only. A WebSocket stays open for each
    /// conversation, so a call skips the connection setup. A call that the WebSocket
    /// cannot serve goes over HTTP. Needs a restart.
    pub websocket: WebSocketChoice,
}

impl Default for OpenAiSettings {
    fn default() -> Self {
        OpenAiSettings {
            originator: DEFAULT_ORIGINATOR.to_owned(),
            models: None,
            subscription_base_url: None,
            api_base_url: None,
            websocket: WebSocketChoice::Auto,
        }
    }
}

/// One entry of `[openai] models`: a model id, or a model id with its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ModelEntry {
    /// A model id, such as `gpt-5.5`.
    Id(String),
    /// A model id with its limits.
    Model(ModelLimits),
}

/// A model of `[openai] models` with its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelLimits {
    /// The model id, such as `gpt-5.5`.
    pub id: String,
    /// The model's context window in tokens, from 1000 to 100000000. For a model of the
    /// catalog, it raises or lowers the catalog's window up to the largest window of
    /// the model; above that, efrd uses the largest one and warns once. Unset: the
    /// catalog's window for the model, else a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The most tokens one model call may produce, from 1 to 1000000. Unset:
    /// `model.max_output_tokens`, else the provider's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl ModelLimits {
    /// The model `id` with no limits.
    pub fn new(id: impl Into<String>) -> Self {
        ModelLimits { id: id.into(), context_window: None, max_output_tokens: None }
    }
}

impl ModelEntry {
    /// The model id.
    pub fn id(&self) -> &str {
        match self {
            ModelEntry::Id(id) => id,
            ModelEntry::Model(model) => &model.id,
        }
    }

    /// The model's context window in tokens, when the entry gives it.
    pub fn context_window(&self) -> Option<u64> {
        match self {
            ModelEntry::Id(_) => None,
            ModelEntry::Model(model) => model.context_window,
        }
    }

    /// The most tokens one model call may produce, when the entry gives it.
    pub fn max_output_tokens(&self) -> Option<u32> {
        match self {
            ModelEntry::Id(_) => None,
            ModelEntry::Model(model) => model.max_output_tokens,
        }
    }
}

impl From<&str> for ModelEntry {
    fn from(id: &str) -> Self {
        ModelEntry::Id(id.to_owned())
    }
}

impl From<ModelLimits> for ModelEntry {
    fn from(model: ModelLimits) -> Self {
        ModelEntry::Model(model)
    }
}

/// `[permissions]`: the permission mode, extra secrets and the user's rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct PermissionSettings {
    /// The default permission mode of a turn: `manual`, `cautious` or `auto`. A prompt
    /// may choose another one for its turn. A turn from the phone runs with at most
    /// `cautious`.
    pub mode: Mode,
    /// Files and directories that count as secrets on top of the built-in ones, so the
    /// model may never read or write them: absolute, or below the home directory as
    /// `~/...`.
    pub secret_paths: Vec<PathBuf>,
    /// The user's rules, `[[permissions.rules]]`, after the built-in rules: the last
    /// rule that matches wins.
    // NOTE: the derived Deserialize skips the rules, and `Settings::parse` reads them
    // one by one, so an error names the rule's place in the file.
    #[serde(deserialize_with = "skip_rules")]
    #[schemars(with = "Policy")]
    pub rules: Policy,
}

impl PermissionSettings {
    /// The machine policy of `mode`: its built-in rules, then the user's.
    pub fn policy(&self, mode: Mode) -> Policy {
        Policy::base(mode).then(self.rules.clone())
    }
}

/// `[shell]`: how hidden shells start and when idle ones stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ShellSettings {
    /// The shell, an absolute path. Unset: `zsh` on the `PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    /// Start the hidden shell as a login shell.
    pub login: bool,
    /// Minutes without output or input, at a prompt and unwatched, before a hidden
    /// shell is closed; 0 keeps idle shells.
    pub idle_minutes: u64,
    /// `keep` leaves sudo's credential cache to sudo; `per_call` makes the hidden shell
    /// forget sudo's and doas's credentials after each call, before anything else runs
    /// there. Read at each call.
    pub sudo_cache: SudoCache,
    /// The longest a command that you approved because it may wait for input at the
    /// terminal (`sudo`, `ssh`) runs before its call answers the model, in minutes,
    /// while a terminal that can type answers follows the conversation; the model's own
    /// timeout holds without one. Read at each call.
    pub interactive_timeout_minutes: u64,
}

impl Default for ShellSettings {
    fn default() -> Self {
        ShellSettings {
            program: None,
            login: true,
            idle_minutes: 60,
            sudo_cache: SudoCache::Keep,
            interactive_timeout_minutes: 60,
        }
    }
}

/// `[conversation]`: queues, approvals, streaming and terminals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ConversationSettings {
    /// The most prompts that may wait behind a running turn.
    pub max_queued: usize,
    /// Seconds an approval request waits before it expires. Unset: it waits until the
    /// user answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_timeout_secs: Option<u64>,
    /// The shortest time between two streamed text updates in the log, in
    /// milliseconds. Live clients see the text sooner, through drafts.
    pub update_interval_ms: u64,
    /// The shortest time between two drafts of a running turn, in milliseconds: the
    /// text, the reasoning and the tool input that a terminal shows as they arrive.
    /// Drafts go only to live clients and never into the log; 0 sends every change.
    pub draft_interval_ms: u64,
    /// Hours without activity after which a terminal's next `,` line starts a new
    /// conversation instead of continuing the old one; 0 continues it forever.
    pub tty_idle_hours: u64,
}

impl Default for ConversationSettings {
    fn default() -> Self {
        ConversationSettings {
            max_queued: 16,
            approval_timeout_secs: None,
            update_interval_ms: 200,
            draft_interval_ms: 16,
            tty_idle_hours: 12,
        }
    }
}

/// Reads the rules as anything and keeps none of them: `Settings::parse` reads them
/// one by one with their place in the file.
fn skip_rules<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Policy, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(Policy::empty())
}

#[cfg(test)]
mod tests;
