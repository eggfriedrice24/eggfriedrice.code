//! The one public error type of the crate, and the only mapping of daemon errors to
//! wire errors.
//!
//! Every method handler returns a [`DaemonError`]; `methods.rs` turns it into the frame
//! the client receives through `From<DaemonError> for ErrorFrame`, which is the one
//! place in the workspace that knows which [`ErrorCode`] each failure is.

use std::io;
use std::path::PathBuf;

use efr_conversation::ConversationError;
use efr_credentials::CredentialsError;
use efr_http::HttpError;
use efr_oauth_openai::OAuthError;
use efr_permissions::PermissionsError;
use efr_protocol::{
    CallId, CommandId, ConversationId, ErrorBody, ErrorCode, ErrorFrame, PtyId, ScopeName,
};
use efr_provider::ProviderError;
use efr_provider_openai::OpenAiError;
use efr_scope::ScopeError;
use efr_screen::ScreenError;
use efr_shell::ShellError;
use efr_stdx::StdxError;
use efr_store::StoreError;
use efr_tools::ToolError;
use efr_transport::TransportError;

/// Every way the daemon, or one of its requests, can fail.
///
/// Variants carry the data a caller acts on; a message names what failed in one
/// sentence and leaves the source's text to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// The efr directories could not be found.
    #[error("the efr directories could not be found")]
    Paths {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },
    /// An `EFR_*` variable holds a value that cannot be used.
    #[error("an EFR_* environment variable could not be read")]
    Env {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },
    /// The config file exists but could not be read.
    #[error("could not read the config file {}", .path.display())]
    ReadConfig {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },
    /// The config file is not valid TOML or has unknown keys.
    #[error("the config file {} is not valid", .path.display())]
    ParseConfig {
        /// The file.
        path: PathBuf,
        /// The parser's error, which names the line.
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A config value is outside its allowed set.
    #[error("the config value {key} = {value:?} is not {expected}")]
    InvalidConfig {
        /// The dotted key.
        key: &'static str,
        /// The value.
        value: String,
        /// What the value must be.
        expected: &'static str,
    },
    /// The operating system's random source could not seed the generator.
    #[error("the random number generator could not be seeded")]
    Random {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },
    /// `HOME` is unset, so the user's home directory is unknown.
    #[error("HOME is not set, so the home directory is unknown")]
    HomeUnknown,
    /// The home directory is unusable.
    #[error("the home directory cannot be used")]
    Home {
        /// The error from `efr-scope`.
        #[source]
        source: ScopeError,
    },
    /// The permission engine's locations could not be built.
    #[error("the permission locations could not be built")]
    Locations {
        /// The error from `efr-permissions`.
        #[source]
        source: PermissionsError,
    },
    /// A file or directory of the daemon could not be created, read or written.
    #[error("could not access {}", .path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },
    /// The lock file could not be locked for a reason other than another daemon.
    #[error("could not lock {}", .path.display())]
    Lock {
        /// The lock file.
        path: PathBuf,
        /// The error from `flock`.
        #[source]
        source: nix::errno::Errno,
    },
    /// Another daemon holds the lock.
    #[error("another efrd holds {}", .path.display())]
    AlreadyRunning {
        /// The lock file.
        path: PathBuf,
    },
    /// The stored daemon id is not a UUID.
    #[error("the daemon id in {} is malformed", .path.display())]
    InvalidDaemonId {
        /// The file.
        path: PathBuf,
    },
    /// A file could not be written atomically.
    #[error("could not write a daemon file")]
    WriteFile {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },
    /// `daemon.json` could not be encoded.
    #[error("could not encode daemon.json")]
    EncodeDiscovery {
        /// The serialiser's error.
        #[source]
        source: serde_json::Error,
    },
    /// The signal handlers could not be installed.
    #[error("could not install the signal handlers")]
    Signals {
        /// The error from tokio.
        #[source]
        source: io::Error,
    },
    /// The store failed.
    #[error("the store failed")]
    Store {
        /// The store's error.
        #[source]
        source: StoreError,
    },
    /// The Unix socket failed.
    #[error("the Unix socket failed")]
    Transport {
        /// The transport's error.
        #[source]
        source: TransportError,
    },
    /// An answer could not be sent to the client.
    #[error("an answer could not be sent to the client")]
    Respond {
        /// The transport's error.
        #[source]
        source: TransportError,
    },
    /// A hidden shell failed.
    #[error("a hidden shell failed")]
    Shell {
        /// The shell manager's error.
        #[source]
        source: ShellError,
    },
    /// A screen failed.
    #[error("a screen failed")]
    Screen {
        /// The screen's error.
        #[source]
        source: ScreenError,
    },
    /// The tool registry could not be built.
    #[error("the tool registry could not be built")]
    Tool {
        /// The registry's error.
        #[source]
        source: ToolError,
    },
    /// The HTTP client could not be built.
    #[error("the HTTP client could not be built")]
    Http {
        /// The HTTP error.
        #[source]
        source: HttpError,
    },
    /// A provider could not be built.
    #[error("a provider could not be built")]
    Provider {
        /// The provider error.
        #[source]
        source: ProviderError,
    },
    /// The OpenAI provider config is invalid.
    #[error("the OpenAI provider settings are invalid")]
    OpenAi {
        /// The provider's error.
        #[source]
        source: OpenAiError,
    },
    /// The subscription login failed.
    #[error("the OpenAI login failed")]
    Login {
        /// The login's error.
        #[source]
        source: OAuthError,
    },
    /// The credential store failed.
    #[error("the credential store failed")]
    Credentials {
        /// The credential store's error.
        #[source]
        source: CredentialsError,
    },
    /// A conversation refused or failed a request.
    #[error("the conversation refused the request")]
    Conversation {
        /// The conversation's error.
        #[source]
        source: ConversationError,
    },
    /// A method's result could not be encoded for the client.
    #[error("the result of {method} could not be encoded")]
    EncodeResult {
        /// The wire method.
        method: &'static str,
        /// The serialiser's error.
        #[source]
        source: serde_json::Error,
    },
    /// A task of the daemon panicked before it answered.
    #[error("the daemon's {task} task panicked")]
    TaskPanicked {
        /// The task.
        task: &'static str,
    },
    /// The connection may not call the method.
    #[error("{method} needs the {scope} scope, which this connection does not hold")]
    Forbidden {
        /// The wire method.
        method: &'static str,
        /// The scope it needs.
        scope: ScopeName,
    },
    /// `hello` reached the dispatcher, which the transport never lets happen.
    #[error("hello was already sent on this connection")]
    HelloRepeated,
    /// The params break a rule that their type cannot express.
    #[error("the request is invalid: {reason}")]
    InvalidParams {
        /// What is wrong.
        reason: &'static str,
    },
    /// A paging cursor was not made by this daemon.
    #[error("the cursor {cursor:?} is not a cursor this daemon made")]
    InvalidCursor {
        /// The cursor.
        cursor: String,
    },
    /// The conversation does not exist.
    #[error("the conversation {conversation_id} does not exist")]
    ConversationNotFound {
        /// The conversation.
        conversation_id: ConversationId,
    },
    /// The conversation has no running turn.
    #[error("the conversation {conversation_id} has no running turn")]
    NoRunningTurn {
        /// The conversation.
        conversation_id: ConversationId,
    },
    /// No approval of this call is pending.
    #[error("the call {call_id} has no pending approval")]
    ApprovalNotPending {
        /// The call.
        call_id: CallId,
    },
    /// The PTY is unknown or its shell has exited.
    #[error("the PTY {pty_id} has no running shell")]
    PtyNotFound {
        /// The PTY.
        pty_id: PtyId,
    },
    /// A command id was used before for another method.
    #[error("the command {command_id} was already used for {method}")]
    CommandReused {
        /// The command id.
        command_id: CommandId,
        /// The method of the stored receipt.
        method: String,
    },
    /// A command that was refused before is refused again, with the stored error.
    #[error("the command was refused before: {}", .body.message)]
    Rejected {
        /// The stored error.
        body: ErrorBody,
    },
}

impl DaemonError {
    /// The wire code of this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            DaemonError::Forbidden { .. } => ErrorCode::Forbidden,
            DaemonError::HelloRepeated | DaemonError::CommandReused { .. } => ErrorCode::Conflict,
            DaemonError::NoRunningTurn { .. } => ErrorCode::Conflict,
            DaemonError::InvalidParams { .. }
            | DaemonError::InvalidCursor { .. }
            | DaemonError::InvalidConfig { .. } => ErrorCode::Invalid,
            DaemonError::ConversationNotFound { .. }
            | DaemonError::ApprovalNotPending { .. }
            | DaemonError::PtyNotFound { .. } => ErrorCode::NotFound,
            DaemonError::AlreadyRunning { .. } => ErrorCode::Busy,
            DaemonError::Rejected { body } => body.code,
            DaemonError::Store { source } => store_code(source),
            DaemonError::Conversation { source } => conversation_code(source),
            DaemonError::Shell { source } => shell_code(source),
            DaemonError::Login { source } => login_code(source),
            DaemonError::Respond { source: TransportError::Overflow { .. } } => ErrorCode::Overflow,
            DaemonError::Paths { .. }
            | DaemonError::Env { .. }
            | DaemonError::ReadConfig { .. }
            | DaemonError::ParseConfig { .. }
            | DaemonError::Random { .. }
            | DaemonError::HomeUnknown
            | DaemonError::Home { .. }
            | DaemonError::Locations { .. }
            | DaemonError::Io { .. }
            | DaemonError::Lock { .. }
            | DaemonError::InvalidDaemonId { .. }
            | DaemonError::WriteFile { .. }
            | DaemonError::EncodeDiscovery { .. }
            | DaemonError::Signals { .. }
            | DaemonError::Transport { .. }
            | DaemonError::Respond { .. }
            | DaemonError::Screen { .. }
            | DaemonError::Tool { .. }
            | DaemonError::Http { .. }
            | DaemonError::Provider { .. }
            | DaemonError::OpenAi { .. }
            | DaemonError::Credentials { .. }
            | DaemonError::EncodeResult { .. }
            | DaemonError::TaskPanicked { .. } => ErrorCode::Internal,
        }
    }

    /// The wire body of this error: its code and its one-sentence message.
    fn body(self) -> ErrorBody {
        match self {
            DaemonError::Rejected { body } => body,
            DaemonError::Respond { source: TransportError::Overflow { last_seq } } => {
                ErrorBody::overflow(last_seq)
            }
            error => {
                let code = error.code();
                let message = match &error {
                    // NOTE: a conversation error's own message is the useful sentence, and
                    // it never carries a secret or a source's text.
                    DaemonError::Conversation { source } => source.to_string(),
                    DaemonError::Login { source } => source.to_string(),
                    other => other.to_string(),
                };
                ErrorBody::new(code, message)
            }
        }
    }
}

/// The single mapping from a daemon error to the frame a client receives.
impl From<DaemonError> for ErrorFrame {
    fn from(error: DaemonError) -> Self {
        ErrorFrame { id: None, error: error.body() }
    }
}

impl From<StoreError> for DaemonError {
    fn from(source: StoreError) -> Self {
        match source {
            StoreError::ApprovalNotPending { call_id, .. } => {
                DaemonError::ApprovalNotPending { call_id }
            }
            StoreError::UnknownConversation { conversation_id } => {
                DaemonError::ConversationNotFound { conversation_id }
            }
            source => DaemonError::Store { source },
        }
    }
}

impl From<ConversationError> for DaemonError {
    fn from(source: ConversationError) -> Self {
        match source {
            ConversationError::NoRunningTurn { conversation_id } => {
                DaemonError::NoRunningTurn { conversation_id }
            }
            ConversationError::ApprovalNotPending { call_id } => {
                DaemonError::ApprovalNotPending { call_id }
            }
            ConversationError::Store { source } => DaemonError::from(source),
            source => DaemonError::Conversation { source },
        }
    }
}

impl From<TransportError> for DaemonError {
    fn from(source: TransportError) -> Self {
        DaemonError::Respond { source }
    }
}

impl From<ShellError> for DaemonError {
    fn from(source: ShellError) -> Self {
        DaemonError::Shell { source }
    }
}

fn store_code(error: &StoreError) -> ErrorCode {
    match error {
        StoreError::Busy { .. } => ErrorCode::Busy,
        StoreError::ApprovalNotPending { .. } | StoreError::UnknownConversation { .. } => {
            ErrorCode::NotFound
        }
        StoreError::DuplicateCommand { .. } | StoreError::ConversationExists { .. } => {
            ErrorCode::Conflict
        }
        // NOTE: a receipt that names a missing event is a daemon bug, not the client's.
        StoreError::ReceiptEventMissing { .. } => ErrorCode::Internal,
        _ => ErrorCode::Internal,
    }
}

fn conversation_code(error: &ConversationError) -> ErrorCode {
    match error {
        ConversationError::Store { source } => store_code(source),
        ConversationError::DuplicateCommand { .. }
        | ConversationError::NoRunningTurn { .. }
        | ConversationError::TurnMismatch { .. } => ErrorCode::Conflict,
        ConversationError::WrongConversation { .. } => ErrorCode::Invalid,
        ConversationError::ApprovalNotPending { .. } => ErrorCode::NotFound,
        ConversationError::QueueFull { .. } => ErrorCode::Busy,
        _ => ErrorCode::Internal,
    }
}

fn shell_code(error: &ShellError) -> ErrorCode {
    match error {
        ShellError::NoShell { .. } => ErrorCode::NotFound,
        _ => ErrorCode::Internal,
    }
}

fn login_code(error: &OAuthError) -> ErrorCode {
    match error {
        OAuthError::LoginInProgress | OAuthError::Bind { .. } => ErrorCode::Busy,
        OAuthError::TimedOut { .. } => ErrorCode::Cancelled,
        OAuthError::Authorization { .. } | OAuthError::TokenRejected { .. } => {
            ErrorCode::Unauthorized
        }
        _ => ErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests;
