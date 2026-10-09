//! The one public error type of the crate, and the only mapping of daemon errors to
//! wire errors.
//!
//! Every method handler returns a [`DaemonError`]; `methods.rs` turns it into the frame
//! the client receives through `From<DaemonError> for ErrorFrame`, which is the one
//! place in the workspace that knows which [`ErrorCode`] each failure is.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use efr_config::{ConfigError, ForeignModel, ModelCompany};
use efr_conversation::ConversationError;
use efr_credentials::CredentialsError;
use efr_http::HttpError;
use efr_oauth_openai::OAuthError;
use efr_permissions::PermissionsError;
use efr_protocol::{
    CallId, CommandId, ConversationId, ErrorBody, ErrorCode, ErrorFrame, PtyId, ScopeName, TurnId,
};
use efr_provider::ProviderError;
use efr_provider_anthropic::AnthropicError;
use efr_provider_openai::OpenAiError;
use efr_scope::{RegistryProblem, ScopeError};
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
    /// The config file could not be read, is not valid, or a variable or flag gives a
    /// value its key cannot hold.
    #[error("the config could not be loaded")]
    Config {
        /// The error from `efr-config`, which names the file, the place and the key.
        #[source]
        source: ConfigError,
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
    /// The socket path is longer than a Unix socket address holds, which a runtime root
    /// deep below `EFR_HOME` can make it.
    #[error("the socket path cannot be used; set EFR_RUNTIME_DIR to a shorter directory")]
    SocketPath {
        /// The error from `efr-stdx`, which names the path and its length.
        #[source]
        source: StdxError,
    },
    /// The config file watcher could not start or watch a directory.
    #[error("could not watch {} for config changes", .path.display())]
    Watch {
        /// The directory.
        path: PathBuf,
        /// The error from inotify.
        #[source]
        source: io::Error,
    },
    /// A tracing filter in `EnvFilter` syntax does not parse.
    #[error("the log filter {filter:?} is not valid")]
    LogFilter {
        /// The filter.
        filter: String,
        /// The parser's error.
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },
    /// The reload task has stopped, because the daemon drains.
    #[error("the config reload task has stopped")]
    ReloadStopped,
    /// The running log filter could not be replaced.
    #[error("the log filter could not be replaced")]
    LogReload {
        /// The error from the reload layer.
        #[source]
        source: tracing_subscriber::reload::Error,
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
    /// The config names a provider that this efrd cannot build. The config's checks
    /// refuse such an id first, so only a daemon of another version gets here.
    #[error("efrd cannot build the provider {id:?}")]
    UnknownProvider {
        /// The provider id of the config.
        id: String,
    },
    /// The OpenAI provider config is invalid.
    #[error("the OpenAI provider settings are invalid")]
    OpenAi {
        /// The provider's error.
        #[source]
        source: OpenAiError,
    },
    /// The Anthropic provider config is invalid.
    #[error("the Anthropic provider settings are invalid")]
    Anthropic {
        /// The provider's error.
        #[source]
        source: AnthropicError,
    },
    /// A login or a logout names a provider that efr does not have.
    #[error(
        "efr has no provider {provider:?}; the providers are openai-subscription, openai-api and anthropic-api"
    )]
    NoSuchProvider {
        /// The provider of the request.
        provider: String,
    },
    /// A login with an API key names a provider that takes none.
    #[error("{provider} takes no API key; log in to it with: efr login openai")]
    NoApiKeyLogin {
        /// The provider of the request.
        provider: String,
    },
    /// An API key that efrd refuses before any check. The key is not kept.
    #[error("the key for {provider} was refused: {problem}")]
    InvalidApiKey {
        /// The provider of the key.
        provider: String,
        /// What is wrong with the key.
        problem: KeyProblem,
    },
    /// `[model] name` is a model of another company than `[model] provider`, and a
    /// prompt that names no model of its own would send it to the provider.
    #[error("{}; {}", foreign(.name, *.company, .provider), foreign(.name, *.company, .provider).fix())]
    ForeignModel {
        /// The value of `[model] name`.
        name: String,
        /// The company of that model.
        company: ModelCompany,
        /// The value of `[model] provider`.
        provider: String,
    },
    /// A prompt needs the model list of the provider, which efrd does not have because
    /// no key is stored.
    #[error("{provider} has no model list yet, because no key is stored; log in with {login}")]
    NoModelListWithoutKey {
        /// The provider.
        provider: String,
        /// The command that stores a key.
        login: &'static str,
    },
    /// A prompt needs the model list of the provider, and its fetch failed.
    #[error("efr could not fetch {list}")]
    ModelListFetch {
        /// The list, as a person names it, such as `Claude's model list`.
        list: &'static str,
        /// The fetch's error, with the server's message.
        #[source]
        source: Arc<ProviderError>,
    },
    /// A prompt needs the model list of the provider, and no fetch gave one.
    #[error("efr could not fetch {list}: {reason}")]
    NoModelList {
        /// The list, as a person names it, such as `Claude's model list`.
        list: &'static str,
        /// Why no list came, in a few words.
        reason: &'static str,
    },
    /// The provider refused an API key, or the check of the key got no answer.
    #[error("the check of the key for {provider} failed")]
    KeyCheck {
        /// The provider of the key.
        provider: String,
        /// The check's error, with the server's message.
        #[source]
        source: ProviderError,
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
    /// The project registry could not be read or changed for a client.
    #[error("the project registry could not be read or changed")]
    Registry {
        /// The error from `efr-scope`, which names the file and what is wrong.
        #[source]
        source: ScopeError,
    },
    /// A project to register is not a directory.
    #[error("{} is not a directory", .path.display())]
    ProjectRootMissing {
        /// The path that was given.
        path: PathBuf,
    },
    /// The root found for a project is the home directory, `/` or a directory above
    /// the home directory, which only an explicit path may register.
    #[error(
        "{} is the home directory, / or above the home directory; name it to register it as a project",
        .root.display()
    )]
    ProjectRootTooWide {
        /// The root.
        root: PathBuf,
    },
    /// No registered project has the root that was given.
    #[error("no project has the root {}", .path.display())]
    ProjectNotRegistered {
        /// The root that was given.
        path: PathBuf,
    },
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
    /// No command runs in the conversation for an answer to reach.
    #[error("no command of conversation {conversation_id} runs for call {call_id}")]
    CallNotRunning {
        /// The conversation.
        conversation_id: ConversationId,
        /// The call the answer was for.
        call_id: CallId,
    },
    /// The call's command does not wait for that input now, so nothing was written.
    #[error("call {call_id} does not wait for this input: {reason}")]
    NotWaitingForInput {
        /// The call the answer was for.
        call_id: CallId,
        /// Why not, never quoting the answer.
        reason: &'static str,
    },
    /// An answer is not one line that can be typed. The reason never quotes it.
    #[error("the answer cannot be typed: {reason}")]
    InvalidAnswer {
        /// What is wrong with it.
        reason: &'static str,
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
    /// The `auto` sandbox cannot run a call now.
    #[error("the sandbox is not available: {reason}")]
    SandboxUnavailable {
        /// The probe's reason.
        reason: String,
    },
    /// A call's sandbox spec could not be written or a path in it could not be planned.
    #[error("the sandbox could not plan the call")]
    SandboxSpec {
        #[source]
        source: efr_sandbox::SandboxError,
    },
    /// The conversation has no finished turn whose changes a client can ask for.
    #[error("the conversation {conversation_id} has no finished turn")]
    NoFinishedTurn {
        /// The conversation.
        conversation_id: ConversationId,
    },
    /// The turn does not exist, or it belongs to another conversation.
    #[error("the turn {turn_id} does not exist in this conversation")]
    TurnNotFound {
        /// The turn.
        turn_id: TurnId,
    },
    /// A method that takes the terminal's conversation came from a connection whose
    /// terminal has no active conversation.
    #[error("this terminal has no active conversation")]
    NoActiveConversation,
    /// The snapshot store could not answer.
    #[error("the snapshot store could not answer")]
    Snapshot {
        #[source]
        source: efr_snapshot::SnapshotError,
    },
    /// The method needs a scope that a process started by the model's commands does
    /// not get.
    #[error("{method} is not open to a process that the model's commands started")]
    ModelSidePeer {
        /// The method.
        method: &'static str,
    },
}

/// What is wrong with an API key that efrd refuses before any check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyProblem {
    /// Nothing was given.
    Empty,
    /// A space, a tab or a line break inside, such as from a paste of two lines.
    Whitespace,
    /// A character outside ASCII, which no key holds.
    NotAscii,
    /// An OpenAI admin key (`sk-admin-`), which cannot call models.
    AdminKey,
}

impl fmt::Display for KeyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyProblem::Empty => "the key is empty",
            KeyProblem::Whitespace => "the key holds whitespace",
            KeyProblem::NotAscii => "the key holds a character outside ASCII",
            KeyProblem::AdminKey => "an admin key cannot call models; use a project key",
        })
    }
}

impl DaemonError {
    /// The wire code of this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            DaemonError::Forbidden { .. } | DaemonError::ModelSidePeer { .. } => {
                ErrorCode::Forbidden
            }
            DaemonError::SandboxUnavailable { .. } => ErrorCode::Conflict,
            DaemonError::SandboxSpec { .. } => ErrorCode::Invalid,
            DaemonError::HelloRepeated | DaemonError::CommandReused { .. } => ErrorCode::Conflict,
            DaemonError::NoRunningTurn { .. } | DaemonError::NotWaitingForInput { .. } => {
                ErrorCode::Conflict
            }
            DaemonError::InvalidParams { .. }
            | DaemonError::ForeignModel { .. }
            | DaemonError::NoSuchProvider { .. }
            | DaemonError::NoApiKeyLogin { .. }
            | DaemonError::InvalidApiKey { .. }
            | DaemonError::InvalidCursor { .. }
            | DaemonError::InvalidAnswer { .. }
            | DaemonError::ProjectRootMissing { .. }
            | DaemonError::ProjectRootTooWide { .. } => ErrorCode::Invalid,
            DaemonError::ProjectNotRegistered { .. } => ErrorCode::NotFound,
            DaemonError::Registry { source } => registry_code(source),
            DaemonError::ConversationNotFound { .. }
            | DaemonError::NoFinishedTurn { .. }
            | DaemonError::TurnNotFound { .. }
            | DaemonError::NoActiveConversation
            | DaemonError::ApprovalNotPending { .. }
            | DaemonError::PtyNotFound { .. }
            | DaemonError::CallNotRunning { .. } => ErrorCode::NotFound,
            DaemonError::AlreadyRunning { .. } => ErrorCode::Busy,
            DaemonError::Rejected { body } => body.code,
            DaemonError::Store { source } => store_code(source),
            DaemonError::Conversation { source } => conversation_code(source),
            DaemonError::Shell { source } => shell_code(source),
            DaemonError::Login { source } => login_code(source),
            DaemonError::KeyCheck { source, .. } => key_check_code(source),
            DaemonError::NoModelListWithoutKey { .. } => ErrorCode::Unauthorized,
            DaemonError::ModelListFetch { source, .. } => summary_code(source),
            DaemonError::Respond { source: TransportError::Overflow { .. } } => ErrorCode::Overflow,
            DaemonError::Paths { .. }
            | DaemonError::Env { .. }
            | DaemonError::Config { .. }
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
            | DaemonError::SocketPath { .. }
            | DaemonError::Watch { .. }
            | DaemonError::LogFilter { .. }
            | DaemonError::LogReload { .. }
            | DaemonError::ReloadStopped
            | DaemonError::Transport { .. }
            | DaemonError::Respond { .. }
            | DaemonError::Screen { .. }
            | DaemonError::Tool { .. }
            | DaemonError::Http { .. }
            | DaemonError::Provider { .. }
            | DaemonError::UnknownProvider { .. }
            | DaemonError::OpenAi { .. }
            | DaemonError::Anthropic { .. }
            | DaemonError::Credentials { .. }
            | DaemonError::EncodeResult { .. }
            | DaemonError::Snapshot { .. }
            | DaemonError::NoModelList { .. }
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
                    // NOTE: the provider's sentence says why a summary request failed
                    // (a rate limit and when to retry, a refused login), as a failed turn
                    // shows it.
                    DaemonError::Conversation {
                        source: conversation @ ConversationError::Summary { source: provider },
                    } => format!("{conversation}: {provider}"),
                    // NOTE: a conversation error's own message is the useful sentence, and
                    // it never carries a secret or a source's text.
                    DaemonError::Conversation { source } => source.to_string(),
                    DaemonError::Login { source } => source.to_string(),
                    // NOTE: the check's error holds the server's message, which says
                    // why a key was refused, and never the key: the check hides a key
                    // that a message quotes.
                    DaemonError::KeyCheck { source, .. } => {
                        format!("{error}: {}", efr_stdx::with_causes(source))
                    }
                    // NOTE: the fetch's error says why the list did not come (no answer,
                    // a refused key with the server's message) and never holds the key.
                    DaemonError::ModelListFetch { source, .. } => {
                        format!("{error}: {}", efr_stdx::with_causes(source))
                    }
                    // NOTE: the registry holds paths and names, no secret, and the
                    // parser's message says what to fix in the file.
                    DaemonError::Registry {
                        source: ScopeError::ParseRegistry { path, source },
                    } => {
                        format!(
                            "could not parse the project registry {}: {}",
                            path.display(),
                            source.message()
                        )
                    }
                    DaemonError::Registry { source } => source.to_string(),
                    // NOTE: the plan's error names a path and a rule of the sandbox,
                    // which is what `efr sandbox explain` must show.
                    DaemonError::SandboxSpec { source } => {
                        format!("the sandbox could not plan the call: {source}")
                    }
                    other => other.to_string(),
                };
                let body = ErrorBody::new(code, message);
                // The choices of an invalid setting travel as data, so a client can
                // offer them.
                match &error {
                    DaemonError::Conversation { source } => match source.data() {
                        Some(data) => body.with_data(data),
                        None => body,
                    },
                    DaemonError::ForeignModel { name, provider, .. } => {
                        body.with_data(serde_json::json!({
                            "setting": "model",
                            "value": name,
                            "provider": provider,
                        }))
                    }
                    _ => body,
                }
            }
        }
    }
}

/// The foreign model `name` of `company` under the provider `provider`, for its text.
fn foreign<'a>(name: &'a str, company: ModelCompany, provider: &'a str) -> ForeignModel<'a> {
    ForeignModel { name, company, provider }
}

/// The wire code of a failed change of the project registry: a broken file or project
/// is the client's to fix, a race is a conflict, the rest is the daemon's.
fn registry_code(source: &ScopeError) -> ErrorCode {
    match source {
        ScopeError::InvalidProject {
            problem: RegistryProblem::DuplicateRoot { .. } | RegistryProblem::DuplicateId { .. },
        }
        | ScopeError::RegistryChanged { .. } => ErrorCode::Conflict,
        ScopeError::InvalidProject { .. }
        | ScopeError::NotAbsolute { .. }
        | ScopeError::ParseRegistry { .. }
        | ScopeError::InvalidRegistry { .. }
        | ScopeError::RegistryShape { .. }
        | ScopeError::DanglingRegistryLink { .. } => ErrorCode::Invalid,
        _ => ErrorCode::Internal,
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
        | ConversationError::TurnMismatch { .. }
        | ConversationError::PromptNotWaiting { .. } => ErrorCode::Conflict,
        ConversationError::WrongConversation { .. } | ConversationError::Unsupported { .. } => {
            ErrorCode::Invalid
        }
        ConversationError::ApprovalNotPending { .. }
        | ConversationError::QuestionNotPending { .. }
        | ConversationError::UnknownTurn { .. }
        | ConversationError::NoQueuedPrompt { .. } => ErrorCode::NotFound,
        ConversationError::RemoteSurfaceAnswer { .. } => ErrorCode::Forbidden,
        ConversationError::QueueFull { .. } => ErrorCode::Busy,
        ConversationError::InvalidSetting { .. } => ErrorCode::Invalid,
        ConversationError::CompactionBusy { .. } | ConversationError::NothingToCompact { .. } => {
            ErrorCode::Conflict
        }
        ConversationError::Summary { source } => summary_code(source),
        _ => ErrorCode::Internal,
    }
}

/// The wire code of a failed summary request: what a client can act on, as for a
/// failed turn.
fn summary_code(error: &ProviderError) -> ErrorCode {
    match error {
        ProviderError::Unauthorized { .. }
        | ProviderError::NotLoggedIn
        | ProviderError::Token { .. } => ErrorCode::Unauthorized,
        ProviderError::RateLimited { .. } | ProviderError::Overloaded { .. } => ErrorCode::Busy,
        ProviderError::UnknownModel { .. } => ErrorCode::Invalid,
        _ => ErrorCode::Internal,
    }
}

fn shell_code(error: &ShellError) -> ErrorCode {
    match error {
        ShellError::NoShell { .. } => ErrorCode::NotFound,
        // NOTE: the answer errors (`NoCall`, `NotWaiting`, `InvalidAnswer`) never reach
        // here: `methods/input_respond.rs` turns them into daemon errors that name the call.
        _ => ErrorCode::Internal,
    }
}

/// The wire code of a failed key check: a key that the provider refused is
/// `unauthorized`, a busy provider `busy`, and a check without an answer `internal`.
fn key_check_code(error: &ProviderError) -> ErrorCode {
    match error {
        ProviderError::Unauthorized { .. }
        | ProviderError::Api { status: Some(400 | 402 | 403), .. } => ErrorCode::Unauthorized,
        ProviderError::RateLimited { .. } | ProviderError::Overloaded { .. } => ErrorCode::Busy,
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
