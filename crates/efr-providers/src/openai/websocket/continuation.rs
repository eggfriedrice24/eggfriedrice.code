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
//!
//! A connection keeps a [`Print`] of each item, not the item: the length and the
//! SHA-256 digest of its JSON text. Two items with the same print are the same bytes
//! on the wire, which is what the server compares. An item that cannot be encoded has no
//! print, and its request sends the whole input and keeps no continuation.

use std::io;

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::openai::convert::ResponsesBody;

/// The last completed answer of one connection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Continuation {
    /// Every field of the request but `input`.
    pub(crate) settings: ResponsesBody,
    /// The prints of the request's whole input, then of the answer's output items.
    pub(crate) input: Vec<Print>,
    /// The answer's id.
    pub(crate) response_id: String,
}

/// One input item as a continuation compares it: the length of its JSON text and the
/// SHA-256 digest of that text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Print {
    len: u64,
    digest: [u8; 32],
}

impl Print {
    /// The print of `item`, or `None` when it cannot be encoded.
    pub(crate) fn of(item: &Value) -> Option<Print> {
        let mut writer = PrintWriter { len: 0, hasher: Sha256::new() };
        serde_json::to_writer(&mut writer, item).ok()?;
        let PrintWriter { len, hasher } = writer;
        Some(Print { len, digest: hasher.finalize().into() })
    }

    /// The prints of `items`, or `None` when one of them cannot be encoded.
    pub(crate) fn all(items: &[Value]) -> Option<Vec<Print>> {
        items.iter().map(Print::of).collect()
    }
}

/// Hashes the JSON text as serde writes it, so no item is copied to a buffer.
///
/// NOTE: the items are partly the model's and the tools' text, so the hash must resist
/// a collision that someone chose. With a collision, a changed history would go as a
/// continuation, and the server would continue a prefix other than the one efr has.
/// SHA-256 resists that; a hash with fixed public keys, such as std's SipHash, does not.
struct PrintWriter {
    len: u64,
    hasher: Sha256,
}

impl io::Write for PrintWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.len = self.len.saturating_add(bytes.len() as u64);
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A request as the continuation compares it, built once per call: the body, its
/// settings and the prints of its input.
#[derive(Debug)]
pub(crate) struct Outgoing<'a> {
    /// The body.
    pub(crate) body: &'a ResponsesBody,
    /// [`ResponsesBody::settings`] of the body.
    pub(crate) settings: ResponsesBody,
    /// The prints of the body's input, or `None` when an item cannot be encoded.
    pub(crate) prints: Option<Vec<Print>>,
}

impl<'a> Outgoing<'a> {
    /// The request of `body`.
    pub(crate) fn of(body: &'a ResponsesBody) -> Outgoing<'a> {
        Outgoing { body, settings: body.settings(), prints: Print::all(body.input()) }
    }
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

/// What `next` sends after `last`, the last completed answer of its connection.
pub(crate) fn plan<'a>(last: Option<&'a Continuation>, next: &Outgoing<'a>) -> Plan<'a> {
    let (Some(last), Some(prints)) = (last, next.prints.as_deref()) else {
        return Plan::Full;
    };
    if last.response_id.is_empty() || last.settings != next.settings {
        return Plan::Full;
    }
    let known = prints.get(..last.input.len());
    match next.body.input().split_at_checked(last.input.len()) {
        Some((_, new)) if known == Some(last.input.as_slice()) => {
            Plan::Incremental { previous: &last.response_id, input: new }
        }
        _ => Plan::Full,
    }
}

#[cfg(test)]
mod tests;
