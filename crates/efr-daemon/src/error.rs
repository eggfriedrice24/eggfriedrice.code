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
    CallId, CommandId, ConversationId, DeviceId, ErrorBody, ErrorCode, ErrorFrame, PairEnd,
    PairingId, PtyId, ScopeName, TurnId,
};
use efr_provider::ProviderError;
use efr_providers::anthropic::AnthropicError;
use efr_providers::openai::OpenAiError;
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
    /// `conversation.delete` without `stop` found a running turn, a queued prompt or a
    /// compaction in the conversation, or found an undo at work, which no delete stops.
    #[error("{}", busy_text(.conversation_id, *.undo))]
    ConversationBusy {
        /// The conversation.
        conversation_id: ConversationId,
        /// True for an undo, which no delete stops.
        undo: bool,
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
    /// The running call cannot move into a background job now (`call.background`), so
    /// nothing moved. The message is the reason alone, which `efr` shows as it is.
    #[error("{reason}")]
    NotMoved {
        /// The call.
        call_id: CallId,
        /// Why not, in words for the person who pressed the key.
        reason: &'static str,
    },
    /// The command that a call left running in the hidden shell cannot be stopped with
    /// `SIGINT` now, so nothing was signalled.
    #[error("the command of call {call_id} cannot be stopped now: {reason}")]
    StopRefused {
        /// The call whose command was to stop.
        call_id: CallId,
        /// Why not.
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
    /// The filtered bus of a call could not start, or its self-test failed (phase E).
    #[error("the bus filter failed: {problem}")]
    BusProxy {
        /// What went wrong.
        problem: BusProxyProblem,
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
    /// The conversation has no job with this id.
    #[error("the conversation {conversation_id} has no job {job_id}")]
    JobNotFound {
        /// The conversation.
        conversation_id: ConversationId,
        /// The job.
        job_id: efr_protocol::JobId,
    },
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
    /// `input.respond` came from a phone. A phone types no input for a command: the
    /// text can be a password, and the buffers of its connection cannot be wiped.
    #[error("a phone does not type input for a command; answer it at the terminal")]
    PhoneInput,
    /// `conversation.resume` came from a connection that is not the shell's or
    /// `efr`'s on the terminal that its context names: a phone, or a made-up tty.
    #[error("a conversation moves only to the terminal that asks, and this is not {tty}")]
    ResumeOffTerminal {
        /// The terminal that the context names.
        tty: String,
    },
    /// Another request changed the terminal's slot, or took the conversation, while
    /// `conversation.resume` ran. Nothing moved.
    #[error("another request changed the conversation of this terminal; try again")]
    ResumeRaced,
    /// A search query has more characters than a query may have.
    #[error("the query is longer than {limit} characters")]
    QueryTooLong {
        /// The most characters of a query.
        limit: usize,
    },
    /// A search query has more words and phrases than a query may have.
    #[error("the query has more than {limit} words")]
    QueryTooManyTerms {
        /// The most words and phrases of a query.
        limit: usize,
    },
    /// No search lane became free while the search could wait.
    #[error("every search lane is busy")]
    SearchBusy,
    /// The search was stopped before it ended.
    #[error("the search was stopped")]
    SearchStopped,
    /// The store failed while it searched. The message never holds the query or a
    /// text of the index, and neither does the log.
    #[error("the search failed in the store")]
    SearchFailed {
        /// The store's error.
        #[source]
        source: StoreError,
    },
    /// A remote connection called a method before its device proved its key.
    #[error("{method} needs a device that proved its key with device.prove first")]
    ProofNeeded {
        /// The method.
        method: &'static str,
    },
    /// A method of the tailnet listener came through the Unix socket.
    #[error("{method} is open only to a device on the tailnet listener")]
    RemoteOnly {
        /// The method.
        method: &'static str,
    },
    /// `device.prove` failed: an unknown or revoked device, a wrong signature, or a
    /// device of another user or tailnet. One message for all, so a peer learns nothing
    /// about which devices exist.
    #[error("the device could not prove its key")]
    ProofRefused,
    /// The device proved its key, but from another Tailscale node than the one that
    /// paired it, such as after Tailscale was installed again on the phone.
    #[error("the device {device_id} connects from another Tailscale node; pair it again")]
    RepairNeeded {
        /// The device.
        device_id: DeviceId,
    },
    /// The connection proved a device before.
    #[error("this connection proved its device before")]
    AlreadyProved,
    /// efrd cannot check a device now, because it starts or stops; try again later.
    #[error("efrd is starting or stopping; try again in {retry_after_secs} seconds")]
    RemoteStarting {
        /// The seconds to wait before the next try.
        retry_after_secs: u32,
    },
    /// A connection of the tailnet listener has no admission value.
    #[error("the remote connection has no admission")]
    RemoteAdmissionMissing,
    /// A step of a device's pairing, or a change of a device, was refused.
    #[error("{problem}")]
    Pairing {
        /// What was wrong.
        problem: PairingProblem,
    },
}

/// Why the filtered bus of a call did not start, or its self-test failed (phase E).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BusProxyProblem {
    /// efrd found no trusted `xdg-dbus-proxy`, or the file changed since efrd started.
    NoProgram,
    /// The launcher's `dbus-proxy` wrapper could not be started.
    StartFailed,
    /// The proxy did not write its ready byte in time, or it ended first.
    NoReadyByte,
    /// The bus is missing, is not a socket, or lies in efr's dirs or a write root.
    BadUpstream {
        /// The bus.
        bus: efr_protocol::BusKind,
    },
    /// The installed `xdg-dbus-proxy` is older than 0.1.9, the version floor.
    OldVersion,
    /// The proxy of the self-test did not end cleanly when its sync pipe closed.
    SelfTest,
    /// The bus rules do not compile.
    Rules,
}

impl fmt::Display for BusProxyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BusProxyProblem::NoProgram => "xdg-dbus-proxy is not installed as a trusted program",
            BusProxyProblem::StartFailed => "the filter program could not start",
            BusProxyProblem::NoReadyByte => "the filter did not become ready",
            BusProxyProblem::BadUpstream { bus: efr_protocol::BusKind::System } => {
                "the system bus is missing or not one that efr accepts"
            }
            BusProxyProblem::BadUpstream { .. } => {
                "the session bus is missing or not one that efr accepts"
            }
            BusProxyProblem::OldVersion => "xdg-dbus-proxy is older than 0.1.9",
            BusProxyProblem::SelfTest => "the filter did not end cleanly in its self-test",
            BusProxyProblem::Rules => "the bus rules do not compile",
        })
    }
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

/// Why efrd refused a step of a device's pairing (`device.pair_start`,
/// `device.enroll`, `device.pair_confirm`) or a change of a device (`device.revoke`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PairingProblem {
    /// `[remote] enabled` is false, so no phone can reach efrd.
    RemoteOff,
    /// The tailnet listener is on in the config, but it does not run now.
    NotListening,
    /// As many pairings as efrd keeps are open.
    TooMany,
    /// `device.enroll` came on a remote connection that has no whois facts of its
    /// peer. The tailnet listener gives every connection its facts, and the Unix
    /// socket refuses the method before it runs (`RemoteOnly`), so it shows a bug.
    NoAdmission,
    /// The token is wrong, used or expired.
    BadToken,
    /// The public key is not an uncompressed P-256 point of 65 bytes.
    BadKey,
    /// A value of the device has a shape that efrd does not keep, such as a name with a
    /// control character.
    BadField {
        /// The value: `name`, `key_fingerprint`, `node_stable_id` or
        /// `tailnet_stable_id`.
        field: &'static str,
    },
    /// A device that is not revoked has the same key.
    KeyInUse {
        /// That device.
        device_id: DeviceId,
    },
    /// The pairing does not wait for an answer of the terminal.
    NotPending {
        /// The pairing.
        pairing_id: PairingId,
    },
    /// The pairing ended without a device.
    Ended {
        /// Why.
        reason: PairEnd,
    },
    /// The phone closed its request before the terminal answered, so no device was
    /// paired.
    PhoneLeft,
    /// No device has this id.
    UnknownDevice {
        /// The id that was given.
        device_id: DeviceId,
    },
    /// The terminal said yes, but the store did not take the device.
    NotStored,
}

impl PairingProblem {
    /// The wire code of the problem.
    fn code(self) -> ErrorCode {
        match self {
            PairingProblem::RemoteOff
            | PairingProblem::KeyInUse { .. }
            | PairingProblem::PhoneLeft => ErrorCode::Conflict,
            PairingProblem::NotListening | PairingProblem::TooMany => ErrorCode::Busy,
            PairingProblem::NoAdmission | PairingProblem::Ended { .. } => ErrorCode::Forbidden,
            PairingProblem::BadToken => ErrorCode::Unauthorized,
            PairingProblem::BadKey | PairingProblem::BadField { .. } => ErrorCode::Invalid,
            PairingProblem::NotPending { .. } | PairingProblem::UnknownDevice { .. } => {
                ErrorCode::NotFound
            }
            PairingProblem::NotStored => ErrorCode::Internal,
        }
    }
}

impl fmt::Display for PairingProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairingProblem::RemoteOff => f.write_str(
                "the tailnet listener is off: set remote.enabled = true in config.toml, then restart efrd",
            ),
            PairingProblem::NotListening => f.write_str(
                "the tailnet listener does not run now: Tailscale is not connected, or efrd waits for it",
            ),
            PairingProblem::TooMany => {
                f.write_str("too many pairings are open; finish or stop one of them first")
            }
            PairingProblem::NoAdmission => {
                f.write_str("the connection has no Tailscale facts of its peer")
            }
            PairingProblem::BadToken => f.write_str("the pairing token is wrong, used or expired"),
            PairingProblem::BadKey => {
                f.write_str("the public key is not an uncompressed P-256 point of 65 bytes")
            }
            PairingProblem::BadField { field } => write!(f, "the device's {field} is not valid"),
            PairingProblem::KeyInUse { device_id } => {
                write!(f, "the device {device_id} has this key; revoke it first")
            }
            PairingProblem::NotPending { pairing_id } => {
                write!(f, "the pairing {pairing_id} does not wait for an answer")
            }
            PairingProblem::Ended { reason: PairEnd::Refused } => {
                f.write_str("the terminal refused the device")
            }
            PairingProblem::Ended { reason: PairEnd::Expired } => {
                f.write_str("the pairing expired before it was finished")
            }
            PairingProblem::Ended { reason: PairEnd::Attempts } => {
                f.write_str("the pairing closed after too many wrong tokens")
            }
            PairingProblem::Ended { .. } => f.write_str("the pairing ended"),
            PairingProblem::PhoneLeft => {
                f.write_str("the phone left before the answer; no device was paired")
            }
            PairingProblem::UnknownDevice { device_id } => {
                write!(f, "no device has the id {device_id}")
            }
            PairingProblem::NotStored => f.write_str("the device could not be stored"),
        }
    }
}

impl DaemonError {
    /// The wire code of this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            DaemonError::Forbidden { .. }
            | DaemonError::ModelSidePeer { .. }
            | DaemonError::PhoneInput
            | DaemonError::ResumeOffTerminal { .. }
            | DaemonError::RemoteOnly { .. } => ErrorCode::Forbidden,
            DaemonError::ProofNeeded { .. }
            | DaemonError::ProofRefused
            | DaemonError::RepairNeeded { .. } => ErrorCode::Unauthorized,
            DaemonError::AlreadyProved => ErrorCode::Conflict,
            DaemonError::RemoteStarting { .. } => ErrorCode::Busy,
            DaemonError::ResumeRaced | DaemonError::SearchBusy => ErrorCode::Busy,
            DaemonError::SearchStopped => ErrorCode::Cancelled,
            DaemonError::QueryTooLong { .. } | DaemonError::QueryTooManyTerms { .. } => {
                ErrorCode::Invalid
            }
            DaemonError::SandboxUnavailable { .. } => ErrorCode::Conflict,
            DaemonError::Pairing { problem } => problem.code(),
            DaemonError::SandboxSpec { .. } => ErrorCode::Invalid,
            DaemonError::HelloRepeated | DaemonError::CommandReused { .. } => ErrorCode::Conflict,
            DaemonError::NoRunningTurn { .. }
            | DaemonError::NotWaitingForInput { .. }
            | DaemonError::StopRefused { .. } => ErrorCode::Conflict,
            DaemonError::InvalidParams { .. }
            | DaemonError::NotMoved { .. }
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
            | DaemonError::JobNotFound { .. }
            | DaemonError::ApprovalNotPending { .. }
            | DaemonError::PtyNotFound { .. }
            | DaemonError::CallNotRunning { .. } => ErrorCode::NotFound,
            DaemonError::AlreadyRunning { .. } | DaemonError::ConversationBusy { .. } => {
                ErrorCode::Busy
            }
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
            | DaemonError::BusProxy { .. }
            | DaemonError::SearchFailed { .. }
            | DaemonError::RemoteAdmissionMissing
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
                    // NOTE: a client asks whether to stop a turn, but never an undo.
                    DaemonError::ConversationBusy { undo, .. } => body.with_data(
                        serde_json::json!({ "busy": if *undo { "undo" } else { "turn" } }),
                    ),
                    // NOTE: the app shows "pair again" for this, not a refusal.
                    DaemonError::RepairNeeded { device_id } => body
                        .with_data(serde_json::json!({ "re_pair": true, "device_id": device_id })),
                    // NOTE: a typed retry hint, so the app waits instead of giving up.
                    DaemonError::RemoteStarting { retry_after_secs } => {
                        body.with_data(serde_json::json!({
                            "starting": true,
                            "retry_after_secs": retry_after_secs,
                        }))
                    }
                    // NOTE: a client names the fix of a listener that is off or down.
                    DaemonError::Pairing { problem: PairingProblem::RemoteOff } => {
                        body.with_data(serde_json::json!({ "remote": "off" }))
                    }
                    DaemonError::Pairing { problem: PairingProblem::NotListening } => {
                        body.with_data(serde_json::json!({ "remote": "not_listening" }))
                    }
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

/// The text of [`DaemonError::ConversationBusy`].
fn busy_text(conversation_id: &ConversationId, undo: bool) -> String {
    if undo {
        format!("an undo runs in the conversation {conversation_id}; a delete waits for its end")
    } else {
        format!(
            "a turn, a queued prompt or a compaction runs in the conversation {conversation_id}"
        )
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
            StoreError::QueryTooLong { limit, .. } => DaemonError::QueryTooLong { limit },
            StoreError::QueryTooManyTerms { limit, .. } => DaemonError::QueryTooManyTerms { limit },
            StoreError::SearchBusy => DaemonError::SearchBusy,
            StoreError::SearchStopped => DaemonError::SearchStopped,
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
        StoreError::Busy { .. } | StoreError::SearchBusy => ErrorCode::Busy,
        StoreError::SearchStopped => ErrorCode::Cancelled,
        StoreError::QueryTooLong { .. } | StoreError::QueryTooManyTerms { .. } => {
            ErrorCode::Invalid
        }
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
        ConversationError::RemoteSurfaceAnswer { .. }
        | ConversationError::RemotePrivilegeAnswer { .. }
        | ConversationError::RemoteAlwaysAnswer { .. }
        | ConversationError::RemoteFullSteer { .. } => ErrorCode::Forbidden,
        ConversationError::NoAlwaysOffer { .. } => ErrorCode::Invalid,
        ConversationError::QueueFull { .. } => ErrorCode::Busy,
        ConversationError::InvalidSetting { .. }
        | ConversationError::ForeignModel { .. }
        | ConversationError::FullModeOff
        | ConversationError::FullModeRefused { .. } => ErrorCode::Invalid,
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
        // NOTE: the answer and stop errors (`NoCall`, `NotWaiting`, `InvalidAnswer`) never
        // reach here: `methods/input_respond.rs` and `methods/shell_interrupt.rs` turn them
        // into daemon errors that name the call.
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
