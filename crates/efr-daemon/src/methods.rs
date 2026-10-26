//! The protocol methods: the `Dispatcher` the transport calls, with the scope each
//! method needs and the handler that answers it.
//!
//! [`scope`] and [`Methods::dispatch`] match `Method` exhaustively, so a method added
//! to `efr-protocol` does not compile here until it has a scope and a handler. Each
//! handler lives in `methods/<noun_verb>.rs` and returns a [`DaemonError`]; the one
//! mapping to the wire is `From<DaemonError> for ErrorFrame`.
//!
//! Scopes: every connection on the Unix socket holds all five, `admin` included,
//! unless its process descends from a hidden shell or shares a hidden shell's session:
//! such a model-side peer, or one whose process is gone, holds `read` only (efr's auto
//! spec, section 13.5; `sandbox/peers.rs`). A connection that declares the phone
//! origin holds `read`, `operate` and `approve`; `terminal` is off by default there and
//! `admin` is never granted. On the Unix socket efrd walks `/proc` for every origin,
//! the phone included, so a model-side process that declares `phone` holds `read` only.
//! A connection of the tailnet listener (`remote.rs`) has no local pid and gets no such
//! walk: before its device proved its key it may call only `hello`, `device.enroll`
//! and `device.prove`, and after that it holds its device's scopes, at most the phone's
//! ([`authorize_remote`]). The Unix socket refuses `device.enroll` and `device.prove`.
//!
//! Presence: a prompt, a steer, an interrupt, a withdraw, an answer, a compaction, an
//! undo, a resume, a stop of a command left running in the hidden shell, a move of a
//! running call to the background and keys for the hidden shell are input of a person, so each one makes
//! its conversation present (`presence.rs`). `Input` takes a side for every method, as the scope does.

use std::sync::Arc;

use efr_protocol::{
    ConversationId, ErrorBody, ErrorCode, ErrorFrame, Hello, HelloResult, Method, Origin,
    PageCursor, ScopeName, Seq,
};
use efr_transport::{ConnectionContext, Dispatcher, Request};
use tracing::Instrument as _;

use crate::DaemonError;
use crate::sandbox::peers::{self, PeerSide};
use crate::state::State;

mod admin_config_reload;
mod admin_login_api_key;
mod admin_login_openai;
mod admin_logout;
mod admin_project_add;
mod admin_project_remove;
mod admin_sandbox_check;
mod admin_status;
mod approval_respond;
mod call_background;
mod conversation_compact;
mod conversation_context;
mod conversation_delete;
mod conversation_diff;
mod conversation_history;
mod conversation_resume;
mod conversation_search;
mod conversation_subscribe;
mod conversations_list;
mod conversations_recent;
mod conversations_search;
mod hello;
mod input_respond;
mod jobs_list;
mod jobs_stop;
mod lease_report;
mod models_list;
mod presence_set;
mod projects_list;
mod prompt_send;
mod prompt_withdraw;
mod pty_attach;
mod pty_resize;
mod pty_write;
mod recent_rows;
mod sandbox_explain;
mod sandbox_surface_respond;
mod search;
mod shell_interrupt;
mod shell_status;
mod turn_interrupt;
mod turn_steer;
mod turn_undo;

pub(crate) use pty_attach::MAX_REPLAY;
pub use search::SearchRowHook;

/// The scopes of a phone connection.
pub(crate) const PHONE_SCOPES: &[ScopeName] =
    &[ScopeName::Read, ScopeName::Operate, ScopeName::Approve];

/// The scopes of a model-side peer and of a peer whose process is gone.
const READ_ONLY: &[ScopeName] = &[ScopeName::Read];

/// The prefix of every paging cursor this daemon makes, so a cursor from elsewhere is
/// refused rather than misread.
const CURSOR_PREFIX: &str = "s:";

/// The daemon's answer to every request.
#[derive(Debug)]
pub(crate) struct Methods {
    state: Arc<State>,
}

impl Methods {
    pub(crate) fn new(state: Arc<State>) -> Self {
        Methods { state }
    }
}

/// The scope a connection needs to call `method`.
pub(crate) fn scope(method: &Method) -> ScopeName {
    match method {
        Method::Hello(_) => ScopeName::Read,
        Method::ConversationsList(_) => ScopeName::Read,
        Method::ConversationSubscribe(_) => ScopeName::Read,
        Method::ConversationHistory(_) => ScopeName::Read,
        Method::LeaseReport(_) => ScopeName::Read,
        Method::ModelsList(_) => ScopeName::Read,
        Method::ProjectsList(_) => ScopeName::Read,
        Method::AdminProjectAdd(_) => ScopeName::Admin,
        Method::AdminProjectRemove(_) => ScopeName::Admin,
        Method::PromptSend(_) => ScopeName::Operate,
        Method::TurnInterrupt(_) => ScopeName::Operate,
        Method::TurnSteer(_) => ScopeName::Operate,
        Method::ApprovalRespond(_) => ScopeName::Approve,
        Method::PtyAttach(_) => ScopeName::Terminal,
        Method::PtyWrite(_) => ScopeName::Terminal,
        Method::PtyResize(_) => ScopeName::Terminal,
        Method::InputRespond(_) => ScopeName::Terminal,
        Method::AdminStatus(_) => ScopeName::Admin,
        Method::AdminConfigReload(_) => ScopeName::Admin,
        Method::AdminLoginOpenAi(_) => ScopeName::Admin,
        Method::AdminLoginApiKey(_) => ScopeName::Admin,
        Method::AdminLogout(_) => ScopeName::Admin,
        Method::SandboxExplain(_) => ScopeName::Read,
        Method::SandboxSurfaceRespond(_) => ScopeName::Approve,
        Method::AdminSandboxCheck(_) => ScopeName::Admin,
        Method::ConversationDiff(_) => ScopeName::Read,
        Method::PromptWithdraw(_) => ScopeName::Operate,
        Method::ConversationCompact(_) => ScopeName::Operate,
        Method::PresenceSet(_) => ScopeName::Operate,
        Method::TurnUndo(_) => ScopeName::Operate,
        Method::JobsList(_) => ScopeName::Read,
        Method::JobsStop(_) => ScopeName::Operate,
        Method::ConversationContext(_) => ScopeName::Read,
        Method::ConversationsRecent(_) => ScopeName::Read,
        Method::ConversationResume(_) => ScopeName::Operate,
        Method::ShellInterrupt(_) => ScopeName::Terminal,
        Method::ShellStatus(_) => ScopeName::Read,
        Method::CallBackground(_) => ScopeName::Terminal,
        Method::ConversationsSearch(_) => ScopeName::Read,
        Method::ConversationSearch(_) => ScopeName::Read,
        Method::ConversationDelete(_) => ScopeName::Terminal,
        Method::DevicePairStart(_) => ScopeName::Admin,
        Method::DevicePairConfirm(_) => ScopeName::Admin,
        Method::DeviceEnroll(_) => ScopeName::Read,
        Method::DeviceProve(_) => ScopeName::Read,
        Method::DeviceList(_) => ScopeName::Admin,
        Method::DeviceRevoke(_) => ScopeName::Admin,
    }
}

/// Where a request is input of a person, which makes the conversation present.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Not input: a read, a login, a change of the daemon, or `presence.set` itself.
    None,
    /// Input in this conversation.
    Conversation(ConversationId),
    /// Input without a conversation id from this terminal: its current conversation.
    Terminal(String),
    /// A prompt without a conversation id from a terminal: `prompt.send` marks the
    /// conversation that routing picks, before and after the prompt, because the
    /// terminal's current one may be another scope's.
    Routed,
}

impl Input {
    /// Where `method`, from the connection of `context`, is input. Every method takes a
    /// side here, so a new one cannot ship without it.
    fn of(state: &State, context: &ConnectionContext, method: &Method) -> Input {
        let conversation = match method {
            Method::PromptSend(params) => match params.conversation_id {
                Some(conversation_id) => Some(conversation_id),
                None => {
                    let hello_tty = state.connections.tty(context.conn_id());
                    return prompt_send::prompt_tty(params, hello_tty)
                        .map_or(Input::None, |_| Input::Routed);
                }
            },
            Method::TurnInterrupt(params) => Some(params.conversation_id),
            Method::TurnSteer(params) => Some(params.conversation_id),
            Method::PromptWithdraw(params) => Some(params.conversation_id),
            Method::ApprovalRespond(params) => Some(params.conversation_id),
            Method::InputRespond(params) => Some(params.conversation_id),
            Method::SandboxSurfaceRespond(params) => Some(params.conversation_id),
            Method::ConversationCompact(params) => Some(params.conversation_id),
            Method::TurnUndo(params) => params.conversation_id,
            // NOTE: a resume is the user's choice of a conversation, so it ends their
            // absence there, as a prompt would.
            Method::ConversationResume(params) => Some(params.conversation_id),
            Method::ShellInterrupt(params) => Some(params.conversation_id),
            // NOTE: Ctrl+B is a key of a person at their own terminal.
            Method::CallBackground(params) => Some(params.conversation_id),
            // NOTE: a stop is the user's input; without a conversation it is the
            // terminal's active one, as for a prompt.
            Method::JobsStop(params) => match params.conversation_id {
                Some(conversation_id) => Some(conversation_id),
                None => {
                    return state
                        .connections
                        .tty(context.conn_id())
                        .map_or(Input::None, Input::Terminal);
                }
            },
            Method::PtyWrite(params) => state.ptys.conversation(params.pty_id),
            Method::Hello(_)
            | Method::ConversationsList(_)
            | Method::ConversationSubscribe(_)
            | Method::ConversationHistory(_)
            | Method::PtyAttach(_)
            | Method::PtyResize(_)
            | Method::LeaseReport(_)
            | Method::ModelsList(_)
            | Method::ProjectsList(_)
            | Method::AdminProjectAdd(_)
            | Method::AdminProjectRemove(_)
            | Method::AdminStatus(_)
            | Method::AdminConfigReload(_)
            | Method::AdminLoginOpenAi(_)
            | Method::AdminLoginApiKey(_)
            | Method::AdminLogout(_)
            | Method::SandboxExplain(_)
            | Method::AdminSandboxCheck(_)
            | Method::ConversationDiff(_)
            | Method::PresenceSet(_)
            | Method::JobsList(_)
            | Method::ConversationContext(_)
            | Method::ConversationsRecent(_)
            | Method::ShellStatus(_)
            | Method::ConversationsSearch(_)
            | Method::ConversationSearch(_) => None,
            // NOTE: a delete is the user's act, but a deleted conversation records no
            // presence: the store refuses its events.
            Method::ConversationDelete(_) => None,
            Method::DevicePairStart(_)
            | Method::DevicePairConfirm(_)
            | Method::DeviceEnroll(_)
            | Method::DeviceProve(_)
            | Method::DeviceList(_)
            | Method::DeviceRevoke(_) => None,
        };
        conversation.map_or(Input::None, Input::Conversation)
    }

    /// The conversation of the input now, when there is one: for a prompt from a
    /// terminal, the terminal's active conversation.
    fn conversation(&self, state: &State) -> Option<ConversationId> {
        match self {
            Input::None | Input::Routed => None,
            Input::Conversation(conversation_id) => Some(*conversation_id),
            Input::Terminal(tty) => {
                state.conversations.current(tty).map(|(_, slot)| slot.conversation_id)
            }
        }
    }

    /// Marks the input before its request runs, in a conversation that the presence
    /// table knows; the changes to record after it.
    fn touch(&self, state: &State) -> Vec<crate::presence::Change> {
        self.conversation(state).map(|id| state.presence.touch(id)).unwrap_or_default()
    }

    /// Marks the input after its request succeeded, in the conversation that it reached.
    async fn mark(&self, state: &State) {
        if let Some(conversation_id) = self.conversation(state) {
            state.presence.input(conversation_id).await;
        }
    }
}

/// The scopes a connection from `surface` holds, whose process stands at `peer`.
///
/// A declared phone gets the phone scopes only when its process is the user's own: the
/// origin is what the client says in `hello`, so a model-side process could declare it.
pub(crate) fn granted(surface: Origin, peer: PeerSide) -> &'static [ScopeName] {
    match (surface, peer) {
        (Origin::Phone, PeerSide::User) => PHONE_SCOPES,
        (_, PeerSide::User) => &ScopeName::ALL,
        _ => READ_ONLY,
    }
}

/// True for a method that needs only `read` but is open to a user-side peer only:
/// search, which would give a process of the model's commands an oracle over the text
/// of every project, and the places of each secret that the index replaced.
pub(crate) fn user_side_only(method: &Method) -> bool {
    matches!(method, Method::ConversationsSearch(_) | Method::ConversationSearch(_))
}

/// Refuses `method` on a connection from `surface` and `peer` that lacks its scope, or
/// that is not the user's own process for a method that only such a peer may call.
pub(crate) fn authorize(
    surface: Origin,
    peer: PeerSide,
    method: &Method,
) -> Result<(), DaemonError> {
    let needed = scope(method);
    if user_side_only(method) && peer != PeerSide::User {
        Err(DaemonError::ModelSidePeer { method: method.name() })
    } else if granted(surface, peer).contains(&needed) {
        Ok(())
    } else if granted(surface, PeerSide::User).contains(&needed) {
        Err(DaemonError::ModelSidePeer { method: method.name() })
    } else {
        Err(DaemonError::Forbidden { method: method.name(), scope: needed })
    }
}

/// True for a method that a remote connection may call before its device proved its
/// key.
pub(crate) fn before_proof(method: &Method) -> bool {
    matches!(method, Method::Hello(_) | Method::DeviceEnroll(_) | Method::DeviceProve(_))
}

/// True for a method that only a connection of the tailnet listener may call.
pub(crate) fn remote_only(method: &Method) -> bool {
    matches!(method, Method::DeviceEnroll(_) | Method::DeviceProve(_))
}

/// Refuses `method` on a remote connection whose device has the scopes `proved`, or
/// that has not proved a device yet (`None`). Before the proof only [`before_proof`]
/// methods pass; after it, a method passes when the device and the phone both hold its
/// scope, so never an `admin` or `terminal` method.
pub(crate) fn authorize_remote(
    proved: Option<&[ScopeName]>,
    method: &Method,
) -> Result<(), DaemonError> {
    if before_proof(method) {
        return Ok(());
    }
    let Some(scopes) = proved else {
        return Err(DaemonError::ProofNeeded { method: method.name() });
    };
    let needed = scope(method);
    if scopes.contains(&needed) && PHONE_SCOPES.contains(&needed) {
        Ok(())
    } else {
        Err(DaemonError::Forbidden { method: method.name(), scope: needed })
    }
}

/// Refuses `method` on the connection of `context`: a remote one by its device's
/// proof, a local one by its scopes ([`authorize`]) and never for a remote-only method.
async fn authorize_context(
    state: &State,
    context: &ConnectionContext,
    method: &Method,
) -> Result<(), DaemonError> {
    if context.remote().is_some() {
        let proved = crate::remote::admission(context).and_then(crate::remote::Admission::proved);
        return authorize_remote(proved.map(|proved| proved.scopes.as_slice()), method);
    }
    if remote_only(method) {
        return Err(DaemonError::RemoteOnly { method: method.name() });
    }
    let peer = side_for(state, context, method).await;
    authorize(context.surface(), peer, method)
}

/// Where the process of `context`'s peer stands: a look at `/proc` off the async
/// workers, for every origin of the Unix socket, where a declared phone is a local
/// process too and gets the same walk. A remote peer has no process here: it stands on
/// the user's side once its device proved its key, and is unknown before.
pub(crate) async fn peer_side(state: &State, context: &ConnectionContext) -> PeerSide {
    if context.remote().is_some() {
        return match crate::remote::prove::proved_device(context) {
            Some(_) => PeerSide::User,
            None => PeerSide::Unknown,
        };
    }
    let pid = context.pid();
    let shells = model_side_roots(state);
    let own = state.pid;
    let unit = state.unit_cgroup.clone();
    tokio::task::spawn_blocking(move || peers::side(pid, &shells, own, unit.as_deref()))
        .await
        .unwrap_or(PeerSide::Unknown)
}

/// The pids below which, or in whose session, a peer is model-side: each hidden
/// shell, each running job's launcher, and the session of each job that ended while a
/// process still had it (efr's auto spec, section 13.5).
pub(crate) fn model_side_roots(state: &State) -> Vec<u32> {
    let mut roots = state.ptys.shell_pids();
    roots.extend(state.jobs.model_side());
    roots
}

/// The side of the peer of `context`, looked at only when `method` needs more than
/// `read`, which every peer holds, or is open to a user-side peer only.
async fn side_for(state: &State, context: &ConnectionContext, method: &Method) -> PeerSide {
    if scope(method) == ScopeName::Read && !user_side_only(method) {
        PeerSide::User
    } else {
        peer_side(state, context).await
    }
}

/// A paging cursor that continues before `seq`.
pub(crate) fn cursor(seq: Seq) -> PageCursor {
    PageCursor::new(format!("{CURSOR_PREFIX}{seq}"))
}

/// The sequence number inside a cursor this daemon made.
pub(crate) fn parse_cursor(cursor: &PageCursor) -> Result<Seq, DaemonError> {
    cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .and_then(|number| number.parse::<u64>().ok())
        .map(Seq::new)
        .ok_or_else(|| DaemonError::InvalidCursor { cursor: cursor.as_str().to_owned() })
}

/// A page size from the request's `limit`: `default` when absent, at most `max`, at
/// least 1.
pub(crate) fn page_size(limit: Option<u32>, default: u32, max: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, max)
}

/// The wire body of `error`, logged once here, where the error is dropped. A client
/// that went away mid-answer is not the daemon's failure.
fn answer(method: &'static str, error: DaemonError) -> ErrorBody {
    let client_gone = matches!(error, DaemonError::Respond { .. });
    let body = ErrorFrame::from(error).error;
    match body.code {
        ErrorCode::Internal if !client_gone => {
            tracing::error!(method, message = %body.message, "request failed");
        }
        _ => tracing::debug!(method, code = %body.code, message = %body.message, "request refused"),
    }
    body
}

impl Dispatcher for Methods {
    async fn hello(
        &self,
        context: &ConnectionContext,
        hello: &Hello,
    ) -> Result<HelloResult, ErrorBody> {
        let peer = peer_side(&self.state, context).await;
        hello::handle(&self.state, context, peer, hello).map_err(|error| answer("hello", error))
    }

    async fn closed(&self, context: &ConnectionContext) {
        self.state.connections.closed(context.conn_id());
    }

    async fn dispatch(&self, request: Request) -> Result<(), ErrorBody> {
        let Request { method, context, responder, cancelled, .. } = request;
        let name = method.name();
        let span = tracing::debug_span!("request", method = name);
        let state = &self.state;
        let handled = async {
            authorize_context(state, &context, &method).await?;
            let input = Input::of(state, &context, &method);
            // NOTE: marked before the handler, so a turn that the input starts never
            // reads the idle state that the input ends; recorded after it, so a prompt
            // is routed by the conversation's last activity before the input.
            let touched = input.touch(state);
            let handled = match method {
                Method::Hello(_) => Err(DaemonError::HelloRepeated),
                Method::ConversationsList(params) => {
                    conversations_list::handle(state, params, &responder).await
                }
                Method::ConversationSubscribe(params) => {
                    Box::pin(conversation_subscribe::handle(
                        state, &context, params, &responder, cancelled,
                    ))
                    .await
                }
                Method::ConversationHistory(params) => {
                    conversation_history::handle(state, params, &responder).await
                }
                Method::PromptSend(params) => {
                    Box::pin(prompt_send::handle(state, &context, params, &responder)).await
                }
                Method::TurnInterrupt(params) => {
                    turn_interrupt::handle(state, &context, params, &responder).await
                }
                Method::TurnSteer(params) => {
                    turn_steer::handle(state, &context, params, &responder).await
                }
                Method::ApprovalRespond(params) => {
                    approval_respond::handle(state, &context, params, &responder).await
                }
                Method::PtyAttach(params) => {
                    Box::pin(pty_attach::handle(state, params, &responder)).await
                }
                Method::PtyWrite(params) => pty_write::handle(state, params, &responder).await,
                Method::PtyResize(params) => pty_resize::handle(state, params, &responder).await,
                Method::InputRespond(params) => {
                    input_respond::handle(state, params, &responder).await
                }
                Method::LeaseReport(params) => {
                    lease_report::handle(state, &context, params, &responder).await
                }
                Method::ModelsList(params) => models_list::handle(state, params, &responder).await,
                Method::ProjectsList(params) => {
                    projects_list::handle(state, params, &responder).await
                }
                Method::AdminProjectAdd(params) => {
                    admin_project_add::handle(state, params, &responder).await
                }
                Method::AdminProjectRemove(params) => {
                    admin_project_remove::handle(state, params, &responder).await
                }
                Method::AdminStatus(params) => {
                    admin_status::handle(state, params, &responder).await
                }
                Method::AdminConfigReload(params) => {
                    admin_config_reload::handle(state, params, &responder).await
                }
                Method::AdminLoginOpenAi(params) => {
                    Box::pin(admin_login_openai::handle(state, params, &responder)).await
                }
                Method::AdminLoginApiKey(params) => {
                    admin_login_api_key::handle(state, params, &responder).await
                }
                Method::AdminLogout(params) => {
                    admin_logout::handle(state, params, &responder).await
                }
                Method::SandboxExplain(params) => {
                    sandbox_explain::handle(state, params, &responder).await
                }
                Method::SandboxSurfaceRespond(params) => {
                    sandbox_surface_respond::handle(state, &context, params, &responder).await
                }
                Method::AdminSandboxCheck(params) => {
                    admin_sandbox_check::handle(state, params, &responder).await
                }
                Method::ConversationDiff(params) => {
                    conversation_diff::handle(state, &context, params, &responder).await
                }
                Method::PromptWithdraw(params) => {
                    prompt_withdraw::handle(state, &context, params, &responder).await
                }
                Method::ConversationCompact(params) => {
                    conversation_compact::handle(state, params, &responder).await
                }
                Method::PresenceSet(params) => {
                    presence_set::handle(state, params, &responder).await
                }
                Method::TurnUndo(params) => {
                    turn_undo::handle(state, &context, params, &responder).await
                }
                Method::JobsList(params) => jobs_list::handle(state, params, &responder).await,
                Method::JobsStop(params) => {
                    jobs_stop::handle(state, &context, params, &responder).await
                }
                Method::ConversationContext(params) => {
                    conversation_context::handle(state, params, &responder).await
                }
                Method::ConversationsRecent(params) => {
                    conversations_recent::handle(state, params, &responder).await
                }
                Method::ConversationResume(params) => {
                    Box::pin(conversation_resume::handle(state, &context, params, &responder)).await
                }
                Method::ShellInterrupt(params) => {
                    shell_interrupt::handle(state, params, &responder).await
                }
                Method::ShellStatus(params) => {
                    shell_status::handle(state, params, &responder).await
                }
                Method::CallBackground(params) => {
                    call_background::handle(state, params, &responder).await
                }
                Method::ConversationsSearch(params) => {
                    conversations_search::handle(state, params, &responder).await
                }
                Method::ConversationSearch(params) => {
                    conversation_search::handle(state, params, &responder).await
                }
                Method::ConversationDelete(params) => {
                    Box::pin(conversation_delete::handle(state, &context, params, &responder)).await
                }
                Method::DeviceProve(params) => {
                    crate::remote::prove::handle(state, &context, params, &responder).await
                }
                // NOTE: the protocol has the device methods before this daemon pairs
                // devices; it refuses them as a daemon from before them does.
                Method::DevicePairStart(_)
                | Method::DevicePairConfirm(_)
                | Method::DeviceEnroll(_)
                | Method::DeviceList(_)
                | Method::DeviceRevoke(_) => Err(DaemonError::InvalidParams {
                    reason: "this efrd does not pair devices yet",
                }),
            };
            state.presence.record(touched).await;
            if handled.is_ok() {
                input.mark(state).await;
            }
            handled
        };
        handled.instrument(span).await.map_err(|error| answer(name, error))
    }
}

#[cfg(test)]
mod tests;
