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
//! spec, section 13.5; `sandbox/peers.rs`). A phone connection (the tailnet listener of
//! a later milestone, where every client is `Origin::Phone`) holds `read`, `operate`
//! and `approve`; `terminal` is off by default there and `admin` is never granted.
//!
//! Presence: a prompt, a steer, an interrupt, a withdraw, an answer, a compaction, an
//! undo and keys for the hidden shell are input of a person, so each one makes its conversation
//! present (`presence.rs`). `Input` takes a side for every method, as the scope does.

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
mod conversation_compact;
mod conversation_diff;
mod conversation_history;
mod conversation_subscribe;
mod conversations_list;
mod hello;
mod input_respond;
mod lease_report;
mod models_list;
mod presence_set;
mod projects_list;
mod prompt_send;
mod prompt_withdraw;
mod pty_attach;
mod pty_resize;
mod pty_write;
mod sandbox_explain;
mod sandbox_surface_respond;
mod turn_interrupt;
mod turn_steer;
mod turn_undo;

/// The scopes of a phone connection.
const PHONE_SCOPES: &[ScopeName] = &[ScopeName::Read, ScopeName::Operate, ScopeName::Approve];

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
    }
}

/// Where a request is input of a person, which makes the conversation present.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Not input: a read, a login, a change of the daemon, or `presence.set` itself.
    None,
    /// Input in this conversation.
    Conversation(ConversationId),
    /// A prompt without a conversation id from this terminal: the conversation is the
    /// terminal's active one after the prompt.
    Terminal(String),
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
                        .map_or(Input::None, Input::Terminal);
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
            | Method::PresenceSet(_) => None,
        };
        conversation.map_or(Input::None, Input::Conversation)
    }

    /// The conversation of the input now, when there is one: for a prompt from a
    /// terminal, the terminal's active conversation.
    fn conversation(&self, state: &State) -> Option<ConversationId> {
        match self {
            Input::None => None,
            Input::Conversation(conversation_id) => Some(*conversation_id),
            Input::Terminal(tty) => {
                state.conversations.active(tty).map(|active| active.conversation_id)
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
pub(crate) fn granted(surface: Origin, peer: PeerSide) -> &'static [ScopeName] {
    match (surface, peer) {
        (Origin::Phone, _) => PHONE_SCOPES,
        (_, PeerSide::User) => &ScopeName::ALL,
        _ => READ_ONLY,
    }
}

/// Refuses `method` on a connection from `surface` and `peer` that lacks its scope.
pub(crate) fn authorize(
    surface: Origin,
    peer: PeerSide,
    method: &Method,
) -> Result<(), DaemonError> {
    let needed = scope(method);
    if granted(surface, peer).contains(&needed) {
        Ok(())
    } else if granted(surface, PeerSide::User).contains(&needed) {
        Err(DaemonError::ModelSidePeer { method: method.name() })
    } else {
        Err(DaemonError::Forbidden { method: method.name(), scope: needed })
    }
}

/// Where the process of `context`'s peer stands: a look at `/proc` off the async
/// workers. A phone has no process here; its surface decides.
pub(crate) async fn peer_side(state: &State, context: &ConnectionContext) -> PeerSide {
    if context.surface() == Origin::Phone {
        return PeerSide::User;
    }
    let pid = context.pid();
    let shells = state.ptys.shell_pids();
    let own = state.pid;
    tokio::task::spawn_blocking(move || peers::side(pid, &shells, own))
        .await
        .unwrap_or(PeerSide::Unknown)
}

/// The side of the peer of `context`, looked at only when `method` needs more than
/// `read`, which every peer holds.
async fn side_for(state: &State, context: &ConnectionContext, method: &Method) -> PeerSide {
    if scope(method) == ScopeName::Read { PeerSide::User } else { peer_side(state, context).await }
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
            let peer = side_for(state, &context, &method).await;
            authorize(context.surface(), peer, &method)?;
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
