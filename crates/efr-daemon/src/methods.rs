//! The protocol methods: the `Dispatcher` the transport calls, with the scope each
//! method needs and the handler that answers it.
//!
//! [`scope`] and [`Methods::dispatch`] match `Method` exhaustively, so a method added
//! to `efr-protocol` does not compile here until it has a scope and a handler. Each
//! handler lives in `methods/<noun_verb>.rs` and returns a [`DaemonError`]; the one
//! mapping to the wire is `From<DaemonError> for ErrorFrame`.
//!
//! Scopes: every connection on the Unix socket holds all five, `admin` included. A
//! phone connection (the tailnet listener of a later milestone, where every client is
//! `Origin::Phone`) holds `read`, `operate` and `approve`; `terminal` is off by default
//! there and `admin` is never granted.

use std::sync::Arc;

use efr_protocol::{
    ErrorBody, ErrorCode, ErrorFrame, Hello, HelloResult, Method, Origin, PageCursor, ScopeName,
    Seq,
};
use efr_transport::{ConnectionContext, Dispatcher, Request};
use tracing::Instrument as _;

use crate::DaemonError;
use crate::state::State;

mod admin_login_openai;
mod admin_status;
mod approval_respond;
mod conversation_history;
mod conversation_subscribe;
mod conversations_list;
mod hello;
mod input_respond;
mod lease_report;
mod models_list;
mod prompt_send;
mod pty_attach;
mod pty_resize;
mod pty_write;
mod turn_interrupt;
mod turn_steer;

/// The scopes of a phone connection.
const PHONE_SCOPES: &[ScopeName] = &[ScopeName::Read, ScopeName::Operate, ScopeName::Approve];

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
        Method::PromptSend(_) => ScopeName::Operate,
        Method::TurnInterrupt(_) => ScopeName::Operate,
        Method::TurnSteer(_) => ScopeName::Operate,
        Method::ApprovalRespond(_) => ScopeName::Approve,
        Method::PtyAttach(_) => ScopeName::Terminal,
        Method::PtyWrite(_) => ScopeName::Terminal,
        Method::PtyResize(_) => ScopeName::Terminal,
        Method::InputRespond(_) => ScopeName::Terminal,
        Method::AdminStatus(_) => ScopeName::Admin,
        Method::AdminLoginOpenAi(_) => ScopeName::Admin,
    }
}

/// The scopes a connection from `surface` holds.
pub(crate) fn granted(surface: Origin) -> &'static [ScopeName] {
    match surface {
        Origin::Phone => PHONE_SCOPES,
        _ => &ScopeName::ALL,
    }
}

/// Refuses `method` on a connection from `surface` that lacks its scope.
pub(crate) fn authorize(surface: Origin, method: &Method) -> Result<(), DaemonError> {
    let needed = scope(method);
    if granted(surface).contains(&needed) {
        Ok(())
    } else {
        Err(DaemonError::Forbidden { method: method.name(), scope: needed })
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
        hello::handle(&self.state, context, hello).map_err(|error| answer("hello", error))
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
            authorize(context.surface(), &method)?;
            match method {
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
                Method::TurnSteer(params) => turn_steer::handle(state, params, &responder).await,
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
                Method::AdminStatus(params) => {
                    admin_status::handle(state, params, &responder).await
                }
                Method::AdminLoginOpenAi(params) => {
                    Box::pin(admin_login_openai::handle(state, params, &responder)).await
                }
            }
        };
        handled.instrument(span).await.map_err(|error| answer(name, error))
    }
}

#[cfg(test)]
mod tests;
