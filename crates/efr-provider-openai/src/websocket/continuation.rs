//! When a request may send only its new input items.
//!
//! With `store: false` the server keeps no answer, except the last answer of an open
//! WebSocket connection: a `response.create` on the same connection may name it as
//! `previous_response_id` and send only the items after it. Codex does that
//! (`codex-rs/core/src/client.rs`, `get_incremental_items` and
//! `prepare_websocket_request`) under these conditions, which efr copies:
//!
//! - the previous answer on this connection completed (`response.completed`);
//! - every field of the request but `input` equals the previous request's;
//! - the new `input` starts with the previous `input` followed by the previous answer's
//!   output items, item for item. Those items come back verbatim in the next request
//!   (the message's `provider_raw`), so an unchanged history passes the test.
//!
//! Anything else sends the whole input without `previous_response_id`: a new
//! connection, an interrupted or failed answer, a changed model, effort or tool list,
//! and a history that a compaction or a rebuild changed, because its start no longer
//! matches.

use serde_json::Value;

use crate::convert::ResponsesBody;

/// The last completed answer of one connection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Continuation {
    /// Every field of the request but `input`.
    pub(crate) settings: ResponsesBody,
    /// The request's whole input, then the answer's output items.
    pub(crate) input: Vec<Value>,
    /// The answer's id.
    pub(crate) response_id: String,
}

/// What a request sends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Plan<'a> {
    /// The whole input.
    Full,
    /// Only `input`, after the answer `previous`.
    Incremental { previous: &'a str, input: &'a [Value] },
}

impl Plan<'_> {
    /// `full` or `incremental`, for the debug lines.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Plan::Full => "full",
            Plan::Incremental { .. } => "incremental",
        }
    }
}

/// What `body` sends after `last`, the last completed answer of its connection.
pub(crate) fn plan<'a>(last: Option<&'a Continuation>, body: &'a ResponsesBody) -> Plan<'a> {
    let Some(last) = last else {
        return Plan::Full;
    };
    if last.response_id.is_empty() || last.settings != body.settings() {
        return Plan::Full;
    }
    match body.input().split_at_checked(last.input.len()) {
        Some((known, new)) if known == last.input.as_slice() => {
            Plan::Incremental { previous: &last.response_id, input: new }
        }
        _ => Plan::Full,
    }
}

#[cfg(test)]
mod tests;
