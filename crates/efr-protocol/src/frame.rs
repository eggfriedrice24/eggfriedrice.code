//! Frames: the JSON objects that travel in each direction.
//!
//! Each frame shape is told apart by which members it has, not by a tag, so the frames
//! read naturally with `jq`: `{"id": 1, "method": "hello", "params": {...}}` and
//! `{"cancel": 1}` from a client; `{"id": 1, "item": {...}}`, `{"id": 1, "end": true}`,
//! `{"id": 1, "error": {...}}` and `{"ack": 1}` from the server. The serde impls are
//! written by hand for that reason.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{ErrorBody, ErrorFrame, Method, ProtocolError, RequestId};

/// A frame from a client to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "a frame is destructured as soon as it is decoded and cancels are rare, so \
              boxing the method would cost an allocation per request to save stack in the \
              rare case"
)]
pub enum ClientFrame {
    /// `{id, method, params}`: start a request.
    Request {
        /// The client's id for the request, unique among its requests in flight.
        id: RequestId,
        /// The method and its params.
        method: Method,
    },
    /// `{cancel: id}`: stop a request in flight. The daemon ends it with a `cancelled`
    /// error, or not at all when it had already ended.
    Cancel {
        /// The request to stop.
        id: RequestId,
    },
}

impl ClientFrame {
    /// Decodes one frame payload. When the payload is not a valid client frame but has
    /// a readable `id`, the error carries it, so the daemon can answer that request with
    /// `invalid` instead of dropping it.
    pub fn from_json(payload: &[u8]) -> Result<Self, ProtocolError> {
        serde_json::from_slice(payload)
            .map_err(|source| ProtocolError::Decode { id: readable_id(payload), source })
    }

    /// The request that the frame starts or cancels.
    pub const fn request_id(&self) -> RequestId {
        match self {
            ClientFrame::Request { id, .. } | ClientFrame::Cancel { id } => *id,
        }
    }
}

/// A frame from the daemon to a client.
///
/// Every request is answered by zero or more [`ServerFrame::Item`] frames and then
/// exactly one [`ServerFrame::End`] or [`ServerFrame::Error`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServerFrame {
    /// `{id, item}`: a result, or one item of a stream. The item's type follows from the
    /// request's method.
    Item {
        /// The request.
        id: RequestId,
        /// The result or stream item as JSON.
        item: Value,
    },
    /// `{id, end: true}`: the request finished; no more frames follow for it.
    End {
        /// The request.
        id: RequestId,
    },
    /// `{id, error}`: the request failed; no more frames follow for it.
    Error(ErrorFrame),
    /// `{ack: id}`: the daemon accepted a streaming request, before its first item, for
    /// methods that document it.
    Ack {
        /// The request.
        id: RequestId,
    },
}

impl ServerFrame {
    /// An item frame holding `value`, which is usually a method's result or item type.
    pub fn item<T: Serialize + ?Sized>(id: RequestId, value: &T) -> Result<Self, ProtocolError> {
        let item =
            serde_json::to_value(value).map_err(|source| ProtocolError::Encode { source })?;
        Ok(ServerFrame::Item { id, item })
    }

    /// The end frame of a request.
    pub const fn end(id: RequestId) -> Self {
        ServerFrame::End { id }
    }

    /// An error frame. `id` is `None` only when the failed frame had no readable id.
    pub const fn error(id: Option<RequestId>, error: ErrorBody) -> Self {
        ServerFrame::Error(ErrorFrame { id, error })
    }

    /// The request that the frame is about; `None` for an error without an id.
    pub const fn request_id(&self) -> Option<RequestId> {
        match self {
            ServerFrame::Item { id, .. } | ServerFrame::End { id } | ServerFrame::Ack { id } => {
                Some(*id)
            }
            ServerFrame::Error(frame) => frame.id,
        }
    }

    /// Decodes one frame payload. The error carries the payload's `id` when it is
    /// readable, so a client can fail that one request.
    pub fn from_json(payload: &[u8]) -> Result<Self, ProtocolError> {
        serde_json::from_slice(payload)
            .map_err(|source| ProtocolError::Decode { id: readable_id(payload), source })
    }
}

/// The `id` member of a payload that failed to decode, when there is a readable one.
fn readable_id(payload: &[u8]) -> Option<RequestId> {
    #[derive(Deserialize)]
    struct IdOnly {
        id: Option<RequestId>,
    }
    serde_json::from_slice::<IdOnly>(payload).ok()?.id
}

#[derive(Serialize)]
struct RequestOut<'a> {
    id: RequestId,
    #[serde(flatten)]
    method: &'a Method,
}

#[derive(Serialize)]
struct CancelOut {
    cancel: RequestId,
}

#[derive(Serialize)]
struct ItemOut<'a> {
    id: RequestId,
    item: &'a Value,
}

#[derive(Serialize)]
struct EndOut {
    id: RequestId,
    end: bool,
}

#[derive(Serialize)]
struct AckOut {
    ack: RequestId,
}

impl Serialize for ClientFrame {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ClientFrame::Request { id, method } => {
                RequestOut { id: *id, method }.serialize(serializer)
            }
            ClientFrame::Cancel { id } => CancelOut { cancel: *id }.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ClientFrame {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut object = Map::<String, Value>::deserialize(deserializer)?;
        if let Some(cancel) = object.remove("cancel") {
            if object.contains_key("method") {
                return Err(de::Error::custom("a client frame has both `cancel` and `method`"));
            }
            let id = RequestId::deserialize(cancel).map_err(de::Error::custom)?;
            return Ok(ClientFrame::Cancel { id });
        }
        let id = object.remove("id").ok_or_else(|| de::Error::missing_field("id"))?;
        let id = RequestId::deserialize(id).map_err(de::Error::custom)?;
        let method = Method::deserialize(Value::Object(object)).map_err(de::Error::custom)?;
        Ok(ClientFrame::Request { id, method })
    }
}

impl Serialize for ServerFrame {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ServerFrame::Item { id, item } => ItemOut { id: *id, item }.serialize(serializer),
            ServerFrame::End { id } => EndOut { id: *id, end: true }.serialize(serializer),
            ServerFrame::Error(frame) => frame.serialize(serializer),
            ServerFrame::Ack { id } => AckOut { ack: *id }.serialize(serializer),
        }
    }
}

/// The members that tell server frames apart; a frame has exactly one of them.
const SERVER_SHAPES: [&str; 4] = ["item", "end", "error", "ack"];

impl<'de> Deserialize<'de> for ServerFrame {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut object = Map::<String, Value>::deserialize(deserializer)?;
        let shapes: Vec<&str> =
            SERVER_SHAPES.into_iter().filter(|key| object.contains_key(*key)).collect();
        let request_id = |object: &mut Map<String, Value>| -> Result<RequestId, D::Error> {
            let id = object.remove("id").ok_or_else(|| de::Error::missing_field("id"))?;
            RequestId::deserialize(id).map_err(de::Error::custom)
        };
        match shapes.as_slice() {
            ["item"] => {
                let id = request_id(&mut object)?;
                let item = object.remove("item").unwrap_or(Value::Null);
                Ok(ServerFrame::Item { id, item })
            }
            ["end"] => {
                if object.get("end") != Some(&Value::Bool(true)) {
                    return Err(de::Error::custom("the `end` member must be `true`"));
                }
                Ok(ServerFrame::End { id: request_id(&mut object)? })
            }
            ["error"] => {
                let frame =
                    ErrorFrame::deserialize(Value::Object(object)).map_err(de::Error::custom)?;
                Ok(ServerFrame::Error(frame))
            }
            ["ack"] => {
                let ack = object.remove("ack").unwrap_or(Value::Null);
                let id = RequestId::deserialize(ack).map_err(de::Error::custom)?;
                Ok(ServerFrame::Ack { id })
            }
            _ => Err(de::Error::custom(
                "a server frame has exactly one of `item`, `end`, `error` and `ack`",
            )),
        }
    }
}

#[cfg(test)]
mod tests;
