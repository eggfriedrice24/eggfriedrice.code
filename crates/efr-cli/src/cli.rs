//! The command line: what `efr` accepts, as clap derive types.
//!
//! The zsh plugin is the main caller, so its four calls are the contract this file
//! must keep. The plugin passes the shell context, the last command line and the prompt
//! in `EFR_CONTEXT`, `EFR_LAST_COMMAND` and `EFR_PROMPT`, so that no other user can
//! read them in the command line:
//!
//! - `efr send`
//! - `efr send --steer` (without `EFR_LAST_COMMAND`)
//! - `efr new`
//! - `efr compact` (`,compact`, with the focus in `EFR_PROMPT`)
//!
//! The flags `--context-json` and `--last-command` and the prompt words do the same by
//! hand, and each wins over its variable.
//!
//! The plugin also hands over the terminal's turn settings, as `EFR_MODE`, `EFR_MODEL`
//! and `EFR_EFFORT`, to `efr send`, `efr new` and `efr settings`. `--mode`, `--model`
//! and `--effort` do the same by hand and win over the variables.

use std::convert::Infallible;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{PossibleValuesParser, TypedValueParser as _};
use clap::{Args, Parser, Subcommand};
use efr_protocol::{ConversationId, Mode, TurnId};

/// The parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "efr",
    version,
    about = "Talk to the efr daemon: send prompts, follow replies, check status",
    long_about = None,
    after_help = "In zsh, the efr plugin runs these for you: `, <prompt>`, `,new`, `,! <text>` and `,compact [focus]`, and `,mode`, `,model` and `,effort` set the terminal's turn settings."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// One `efr` command.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Send a prompt to this terminal's conversation and follow the reply.
    Send(SendArgs),
    /// Start a new conversation for this terminal with its first prompt.
    New(NewArgs),
    /// Show whether the daemon runs, and its health.
    Status,
    /// List recent conversations, or show the events of one.
    History(HistoryArgs),
    /// Show what the last turn changed in the files of its project and $SCRATCH.
    Diff(DiffArgs),
    /// Make room in the model's context of this terminal's conversation now.
    ///
    /// efrd writes a summary of the earlier turns and keeps the newest ones word for
    /// word. It starts no turn.
    Compact(CompactArgs),
    /// Show the mode, model and effort that a prompt would use, with where each comes
    /// from.
    Settings(TurnSettingsArgs),
    /// List the models that a prompt may name.
    Models(ModelsArgs),
    /// Log in to a model provider: the OpenAI subscription in a browser, or an API key.
    #[command(subcommand)]
    Login(LoginCommand),
    /// Forget the stored login of a model provider.
    Logout(LogoutArgs),
    /// Show, check, edit and change config.toml.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// List, add and remove the projects that the permission modes trust.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Show where efr keeps its files, and where the daemon keeps them.
    Paths(PathsArgs),
    /// Check the sandbox of the auto mode, or explain what it does with a path.
    #[command(subcommand)]
    Sandbox(SandboxCommand),
}

/// The `efr sandbox` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum SandboxCommand {
    /// Run the sandbox checks now and print each one with its result and fix.
    Check,
    /// Show whether a command in the auto sandbox can read and write PATH, and why.
    Explain {
        /// The path; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

/// The arguments of `efr paths`.
#[derive(Debug, Args)]
pub(crate) struct PathsArgs {
    /// Print the same facts as JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The arguments of `efr send`.
#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// Add the text to the running turn instead of queueing a new prompt. A running
    /// turn keeps its settings, so a steer takes none.
    #[arg(long, conflicts_with_all = ["mode", "model", "effort"])]
    pub(crate) steer: bool,

    /// The user's shell as the zsh plugin observed it, as a JSON object [env:
    /// EFR_CONTEXT]
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored [env: EFR_LAST_COMMAND]
    #[arg(long, value_name = "TEXT", conflicts_with = "steer")]
    pub(crate) last_command: Option<LastCommand>,

    /// Send to this conversation instead of the terminal's active one.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<ConversationId>,

    #[command(flatten)]
    pub(crate) settings: TurnSettingsArgs,

    /// The prompt; the words are joined with spaces [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// The arguments of `efr new`.
#[derive(Debug, Args)]
pub(crate) struct NewArgs {
    /// The user's shell as the zsh plugin observed it, as a JSON object [env:
    /// EFR_CONTEXT]
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored [env: EFR_LAST_COMMAND]
    #[arg(long, value_name = "TEXT")]
    pub(crate) last_command: Option<LastCommand>,

    #[command(flatten)]
    pub(crate) settings: TurnSettingsArgs,

    /// The first prompt of the new conversation; the words are joined with spaces
    /// [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// The turn settings that a prompt asks for. Each one left out comes from its
/// variable, and without that from the daemon's config.
#[derive(Debug, Clone, Default, Args)]
pub(crate) struct TurnSettingsArgs {
    /// The permission mode of the turn [env: EFR_MODE]
    #[arg(long, value_name = "MODE", value_parser = mode_parser())]
    pub(crate) mode: Option<Mode>,

    /// The model of the turn, one that `efr models` lists [env: EFR_MODEL]
    #[arg(long, value_name = "MODEL")]
    pub(crate) model: Option<String>,

    /// The reasoning effort of the turn, one that the model takes [env: EFR_EFFORT]
    #[arg(long, value_name = "EFFORT")]
    pub(crate) effort: Option<String>,
}

/// Reads a mode by its wire name, and lists the names in the help and in the error.
fn mode_parser() -> impl clap::builder::TypedValueParser<Value = Mode> {
    PossibleValuesParser::new(Mode::ALL.map(Mode::as_str)).try_map(|name| name.parse::<Mode>())
}

/// The arguments of `efr models`.
#[derive(Debug, Args)]
pub(crate) struct ModelsArgs {
    /// Print only the model ids, one per line, for completion.
    #[arg(long)]
    pub(crate) names: bool,
}

/// A command line from the user's shell, as `--last-command` or `EFR_LAST_COMMAND`
/// gives it. It can hold a secret (`export TOKEN=...`), so `Debug` shows only its
/// length.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LastCommand(String);

impl LastCommand {
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for LastCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LastCommand(<{} bytes>)", self.0.len())
    }
}

impl From<String> for LastCommand {
    fn from(text: String) -> Self {
        LastCommand(text)
    }
}

impl FromStr for LastCommand {
    type Err = Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(LastCommand(text.to_owned()))
    }
}

/// The arguments of `efr history`.
#[derive(Debug, Args)]
pub(crate) struct HistoryArgs {
    /// The conversation to show: its id, or the start of it. Without one, the recent
    /// conversations are listed; with --verbose, the newest conversation of this
    /// terminal shows, else the newest of all.
    #[arg(value_name = "CONVERSATION")]
    pub(crate) conversation: Option<String>,

    /// The most conversations or events to show.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<u32>,

    /// Continue from a cursor that an earlier page printed.
    #[arg(long, value_name = "CURSOR")]
    pub(crate) cursor: Option<String>,

    /// Also show the facts that efr recorded for each exit from the auto sandbox.
    #[arg(long, short = 'v')]
    pub(crate) verbose: bool,
}

/// The arguments of `efr diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    /// Show this turn instead of the last one.
    #[arg(long, value_name = "ID")]
    pub(crate) turn: Option<TurnId>,

    /// The conversation whose last turn shows: its id, or the start of it. Without it,
    /// this terminal's conversation shows, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,

    /// List the files that changed, with their counts of lines, instead of the diff.
    #[arg(long)]
    pub(crate) stat: bool,
}

/// The arguments of `efr compact`.
#[derive(Debug, Args)]
pub(crate) struct CompactArgs {
    /// The conversation to compact: its id, or the start of it. Without it, this
    /// terminal's conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,

    /// What the summary must keep, such as: the failing test and its fix; the words are
    /// joined with spaces [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "FOCUS")]
    pub(crate) focus: Vec<String>,
}

/// The providers `efr login` knows.
#[derive(Debug, Subcommand)]
pub(crate) enum LoginCommand {
    /// Log in to the OpenAI subscription in a browser. The daemon runs the login; this
    /// prints the URL to open and waits for the browser to finish.
    Openai,
    /// Store an OpenAI API key for the openai-api provider.
    ///
    /// On a terminal, efr asks for the key and does not show it; otherwise it reads the
    /// key from stdin. Never put a key on the command line: other users can read it in
    /// the process list, and it stays in the shell's history.
    OpenaiApi(KeyArgs),
    /// Store an Anthropic API key for the anthropic-api provider.
    ///
    /// On a terminal, efr asks for the key and does not show it; otherwise it reads the
    /// key from stdin. Never put a key on the command line: other users can read it in
    /// the process list, and it stays in the shell's history.
    #[command(alias = "anthropic-api")]
    Anthropic(KeyArgs),
}

/// The arguments of `efr login openai-api` and `efr login anthropic`.
#[derive(Debug, Args)]
pub(crate) struct KeyArgs {
    /// Read the key from OPENAI_API_KEY or ANTHROPIC_API_KEY of this shell. The daemon
    /// never reads these variables.
    #[arg(long)]
    pub(crate) from_env: bool,

    /// Store the key without a check with the provider, such as on a computer that is
    /// offline.
    #[arg(long)]
    pub(crate) no_check: bool,
}

/// The arguments of `efr logout`.
#[derive(Debug, Args)]
pub(crate) struct LogoutArgs {
    /// The provider: openai (the subscription), openai-api or anthropic.
    #[arg(value_name = "PROVIDER", value_parser = provider_parser())]
    pub(crate) provider: ProviderName,
}

/// A model provider by the name that a user types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderName {
    /// The OpenAI subscription: `openai` or `openai-subscription`.
    OpenAi,
    /// OpenAI with an API key: `openai-api`.
    OpenAiApi,
    /// Anthropic with an API key: `anthropic` or `anthropic-api`.
    Anthropic,
}

impl ProviderName {
    /// Every name, the short ones first.
    const NAMES: [(&'static str, ProviderName); 5] = [
        ("openai", ProviderName::OpenAi),
        ("openai-api", ProviderName::OpenAiApi),
        ("anthropic", ProviderName::Anthropic),
        ("openai-subscription", ProviderName::OpenAi),
        ("anthropic-api", ProviderName::Anthropic),
    ];

    /// The provider's id, as the daemon and the config name it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            ProviderName::OpenAi => "openai-subscription",
            ProviderName::OpenAiApi => "openai-api",
            ProviderName::Anthropic => "anthropic-api",
        }
    }
}

/// Reads a provider by one of its names, and lists the names in the help and the
/// error.
fn provider_parser() -> impl clap::builder::TypedValueParser<Value = ProviderName> {
    PossibleValuesParser::new(ProviderName::NAMES.map(|(name, _)| name)).try_map(|name| {
        ProviderName::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, provider)| *provider)
            .ok_or_else(|| format!("no provider is named {name:?}"))
    })
}

/// The `efr project` commands, which change projects.toml through the daemon.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCommand {
    /// List the registered projects with their roots.
    List,
    /// Register a project: PATH, or without one the git work tree that holds the
    /// current directory (else the directory itself).
    Add {
        /// The project's root directory; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// A name for the project; the last part of the root by default.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
    /// Take the project with this root out of the registry.
    Remove {
        /// The project's root directory; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

/// The `efr config` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print every setting with where it comes from, then what the daemon reads.
    Show,
    /// Check a config file; errors name the line, the column and the key.
    Check {
        /// The file to check; config.toml in the config root by default.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
    /// Open config.toml in $VISUAL or $EDITOR (vi by default), check it, then reload it.
    Edit,
    /// Print the JSON schema of config.toml.
    Schema,
    /// Ask the daemon to read config.toml again now.
    Reload,
    /// Set one key, such as model.name, keeping the file's comments and layout.
    Set {
        /// The dotted key, such as model.name or conversation.max_queued.
        #[arg(value_name = "KEY")]
        key: String,
        /// The value: text, a number, true or false, or a list as words separated by
        /// commas.
        #[arg(value_name = "VALUE", allow_hyphen_values = true)]
        value: String,
    },
    /// Remove one key, so its default applies again.
    Unset {
        /// The dotted key.
        #[arg(value_name = "KEY")]
        key: String,
    },
}

#[cfg(test)]
mod tests;
