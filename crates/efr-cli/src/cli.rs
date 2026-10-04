//! The command line: what `efr` accepts, as clap derive types.
//!
//! The zsh plugin is the main caller, so its three calls are the contract this file
//! must keep:
//!
//! - `efr send --context-json <json> [--last-command <text>] -- <prompt words>`
//! - `efr send --steer --context-json <json> -- <text>`
//! - `efr new --context-json <json> [--last-command <text>] -- <prompt words>`

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};
use efr_protocol::ConversationId;

/// The parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "efr",
    version,
    about = "Talk to the efr daemon: send prompts, follow replies, check status",
    long_about = None,
    after_help = "In zsh, the efr plugin runs these for you: `, <prompt>`, `,new` and `,! <text>`."
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
    /// Log in to a model provider.
    #[command(subcommand)]
    Login(LoginCommand),
    /// Show settings.
    #[command(subcommand)]
    Config(ConfigCommand),
}

/// The arguments of `efr send`.
#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// Add the text to the running turn instead of queueing a new prompt.
    #[arg(long)]
    pub(crate) steer: bool,

    /// The user's shell as the zsh plugin observed it, as a JSON object.
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored.
    #[arg(long, value_name = "TEXT", conflicts_with = "steer")]
    pub(crate) last_command: Option<LastCommand>,

    /// Send to this conversation instead of the terminal's active one.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<ConversationId>,

    /// The prompt; the words are joined with spaces.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// The arguments of `efr new`.
#[derive(Debug, Args)]
pub(crate) struct NewArgs {
    /// The user's shell as the zsh plugin observed it, as a JSON object.
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored.
    #[arg(long, value_name = "TEXT")]
    pub(crate) last_command: Option<LastCommand>,

    /// The first prompt of the new conversation; the words are joined with spaces.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// A command line from the user's shell, as `--last-command` gives it. It can hold a
/// secret (`export TOKEN=...`), so `Debug` shows only its length.
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
    /// conversations are listed.
    #[arg(value_name = "CONVERSATION")]
    pub(crate) conversation: Option<String>,

    /// The most conversations or events to show.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<u32>,

    /// Continue from a cursor that an earlier page printed.
    #[arg(long, value_name = "CURSOR")]
    pub(crate) cursor: Option<String>,
}

/// The providers `efr login` knows.
#[derive(Debug, Subcommand)]
pub(crate) enum LoginCommand {
    /// Log in to the OpenAI subscription in a browser. The daemon runs the login; this
    /// prints the URL to open and waits for the browser to finish.
    Openai,
}

/// The `efr config` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the effective client settings and where each one comes from.
    Show,
}

#[cfg(test)]
mod tests;
