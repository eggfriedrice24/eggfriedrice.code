//! The one error type of the CLI, and the exit code each failure maps to.

use std::io;

use efr_client::ClientError;
use efr_config::ConfigError;
use efr_protocol::{ErrorBody, ErrorCode, Mode};
use efr_stdx::StdxError;
use efr_stdx::env::Var;

use crate::turn_settings::SettingSource;

/// The next step when the model provider has no usable credentials.
pub(crate) const LOGIN_HINT: &str = "log in with: efr login openai";

/// The next step when the provider refuses the model. Which model ids the
/// subscription serves to efr is unknown until it answers, so a first run may meet it.
pub(crate) const MODEL_HINT: &str = "choose another model: ,model <model> for this terminal, or name = \"<model>\" under [model] in config.toml, which the daemon reloads without a restart";

/// How `efr` exits. The zsh plugin and scripts tell the cases apart by the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// 0: the command did what it was asked.
    Success,
    /// 1: the daemon failed the request, the turn failed, or the connection broke.
    DaemonError,
    /// 1: a config file has an error (`efr config check`, `edit`, `set`, `unset`), or
    /// the daemon refused one on `efr config reload`.
    Invalid,
    /// 2: the command line or its input is wrong.
    Usage,
    /// 3: no daemon listens on the socket.
    NotRunning,
    /// 130: the user pressed Ctrl+C, the shell convention for an interrupt.
    Interrupted,
}

impl Exit {
    /// The process exit code.
    pub(crate) const fn code(self) -> u8 {
        match self {
            Exit::Success => 0,
            Exit::DaemonError | Exit::Invalid => 1,
            Exit::Usage => 2,
            Exit::NotRunning => 3,
            Exit::Interrupted => 130,
        }
    }
}

/// Every way an `efr` command can fail.
///
/// The message says what failed in one sentence; [`report`](crate::run) adds the
/// sources after it and a hint where there is a useful next step.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// The XDG directories, and so the socket, could not be found.
    #[error("the efr directories could not be found")]
    Dirs {
        #[source]
        source: StdxError,
    },

    /// An `EFR_*` variable holds a value that cannot be used.
    #[error("an environment variable cannot be used")]
    Environment {
        #[source]
        source: StdxError,
    },

    /// The system random number generator could not be seeded for command ids.
    #[error("the random number generator could not be seeded")]
    Random {
        #[source]
        source: StdxError,
    },

    /// Talking to the daemon failed, including when no daemon is running.
    #[error(transparent)]
    Client(#[from] ClientError),

    /// `--context-json` or `EFR_CONTEXT`, named by `input`, is not the JSON object the
    /// zsh plugin sends.
    #[error("{input} is not a shell context object")]
    InvalidContext {
        input: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// `EFR_MODE` names no mode. A flag with no mode is clap's usage error.
    #[error("{} names the mode {value:?}, which does not exist; choose one of: {}", .input.name(), Mode::ALL.map(Mode::as_str).join(", "))]
    UnknownMode { input: Var, value: String },

    /// The daemon's model list has no model with this id.
    #[error("the daemon has no model {model:?} ({from}); choose one of: {}", .choices.join(", "))]
    UnknownModel { model: String, from: SettingSource, choices: Vec<String> },

    /// The model does not take the effort.
    #[error("{model} does not take the effort {effort:?} ({from}); choose one of: {}", .choices.join(", "))]
    UnknownEffort { model: String, effort: String, from: SettingSource, choices: Vec<String> },

    /// The effort, for a model whose efforts efr does not know, is not one word of an
    /// effort's form, which the daemon refuses for every model.
    #[error("{model} does not take the effort {effort:?} ({from}); an effort is one word of at most {max} lowercase letters, digits, - and _", max = efr_protocol::EFFORT_MAX_LEN)]
    EffortNotAWord { model: String, effort: String, from: SettingSource },

    /// The daemon's model list marks no model as the default, so a prompt that names
    /// none has no model.
    #[error("the daemon marks no model as its default")]
    NoDefaultModel,

    /// The current directory is unknown, so a relative path or the default project
    /// cannot be found.
    #[error("the current directory is unknown; name the directory with an absolute path")]
    NoWorkingDirectory,

    /// There is no prompt text to send.
    #[error("the prompt is empty")]
    EmptyPrompt,

    /// `efr new` came without the first prompt that a conversation needs.
    #[error("efr new needs the first prompt of the new conversation")]
    NewWithoutPrompt,

    /// `efr send --steer` cannot tell which conversation to steer.
    #[error("efr send --steer needs --conversation, or a shell context with a tty")]
    SteerNeedsConversation,

    /// The terminal has no active conversation to steer.
    #[error("no conversation is active in {tty}")]
    NoActiveConversation { tty: String },

    /// No conversation id starts with what the user typed.
    #[error("no conversation matches {query:?}")]
    ConversationNotFound { query: String },

    /// More than one conversation id starts with what the user typed.
    #[error("{query:?} matches {matches} conversations; type more of the id")]
    AmbiguousConversation { query: String, matches: usize },

    /// The turn ended with an error from the daemon or the model provider.
    #[error("the turn failed with {}: {}", .body.code, .body.message)]
    TurnFailed { body: ErrorBody },

    /// Another client interrupted the turn.
    #[error("the turn was interrupted")]
    TurnInterrupted,

    /// The daemon restarted while the turn ran, so it was cancelled.
    #[error("the turn was cancelled because the daemon restarted")]
    TurnCancelled,

    /// The daemon ended the subscription before the turn ended.
    #[error("the daemon stopped sending the turn's events before it ended")]
    SubscriptionEnded,

    /// The subscription fell behind again and again; the terminal cannot keep up.
    #[error("the turn's events arrived faster than they could be shown, {times} times")]
    FellBehind { times: u32 },

    /// The login stream ended without a completed login.
    #[error("the daemon ended the login before it completed")]
    LoginIncomplete,

    /// The user pressed Ctrl+C.
    #[error("interrupted")]
    Interrupted,

    /// Writing to stdout failed, usually because the reader of a pipe went away.
    #[error("the output could not be written")]
    Output {
        #[source]
        source: io::Error,
    },

    /// Reading keys from the terminal failed.
    #[error("the terminal could not be read")]
    Terminal {
        #[source]
        source: io::Error,
    },

    /// A config file has an error, which the command has already printed with its
    /// place.
    #[error("the config file has an error")]
    ConfigInvalid,

    /// The config file could not be read, created or written.
    #[error("config.toml could not be changed")]
    ConfigFile {
        #[source]
        source: Box<ConfigError>,
    },

    /// The editor of `efr config edit` could not be started.
    #[error("the editor {editor:?} could not be started")]
    Editor {
        editor: String,
        #[source]
        source: io::Error,
    },

    /// The editor of `efr config edit` failed, so the file is not checked.
    #[error("the editor {editor:?} exited with {status}")]
    EditorFailed { editor: String, status: std::process::ExitStatus },
}

impl CliError {
    /// The exit code for this failure.
    pub(crate) fn exit(&self) -> Exit {
        match self {
            CliError::Client(ClientError::DaemonNotRunning { .. }) => Exit::NotRunning,
            CliError::InvalidContext { .. }
            | CliError::UnknownMode { .. }
            | CliError::UnknownModel { .. }
            | CliError::UnknownEffort { .. }
            | CliError::EffortNotAWord { .. }
            | CliError::EmptyPrompt
            | CliError::NewWithoutPrompt
            | CliError::SteerNeedsConversation
            | CliError::NoWorkingDirectory
            | CliError::AmbiguousConversation { .. } => Exit::Usage,
            CliError::Interrupted => Exit::Interrupted,
            CliError::ConfigInvalid => Exit::Invalid,
            _ => Exit::DaemonError,
        }
    }

    /// A next step for the user, printed on its own line after the error. The ones a
    /// first run meets most (no daemon, no login) name the command that fixes them.
    pub(crate) fn hint(&self) -> Option<&'static str> {
        match self {
            CliError::Client(ClientError::DaemonNotRunning { .. }) => Some(
                "start the daemon with: systemctl --user start efrd, or `just run` in the efr checkout for a foreground one",
            ),
            CliError::Client(
                ClientError::ConnectTimedOut { .. } | ClientError::HelloTimedOut { .. },
            ) => Some("the daemon is not answering; its log: journalctl --user -u efrd"),
            CliError::Client(ClientError::ProtocolMismatch { .. }) => {
                Some("efr and efrd come from different builds; install both from one build")
            }
            CliError::Dirs { source: StdxError::RuntimeDirUnset } => Some(
                "a systemd login sets XDG_RUNTIME_DIR; without one, set EFR_HOME or EFR_RUNTIME_DIR",
            ),
            CliError::Dirs { source: StdxError::HomeNotFound } => Some("set HOME, or EFR_HOME"),
            // A turn fails as unauthorized when the provider has no usable credentials.
            CliError::TurnFailed { body } if body.code == ErrorCode::Unauthorized => {
                Some(LOGIN_HINT)
            }
            // The daemon names the refused model in the data of an `invalid` turn: the
            // provider's refusal has a `model`, a turn setting that no longer fits has a
            // `setting`, which names the model for an effort too.
            CliError::TurnFailed { body }
                if body.code == ErrorCode::Invalid
                    && body.data.as_ref().is_some_and(|data| match data.get("setting") {
                        Some(setting) => setting == "model",
                        None => data.get("model").is_some(),
                    }) =>
            {
                Some(MODEL_HINT)
            }
            CliError::NewWithoutPrompt => {
                Some("in zsh, a bare ,new makes the next , line start a new conversation")
            }
            _ => None,
        }
    }

    /// True when the failure needs no message: the reader of stdout went away, or the
    /// user interrupted and already sees that.
    pub(crate) fn is_silent(&self) -> bool {
        match self {
            CliError::Output { source } => source.kind() == io::ErrorKind::BrokenPipe,
            CliError::Interrupted | CliError::ConfigInvalid => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
