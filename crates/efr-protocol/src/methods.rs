//! One module per wire method, and the types that several methods share.
//!
//! A method `noun.verb` lives in `noun_verb.rs`, its params type is `NounVerb`, and its
//! answer is `NounVerbResult` for a unary method or `NounVerbItem` for a streaming one.
//! A unary method answers with exactly one item frame; a streaming method sends any
//! number of item frames. Both then end with `{id, end: true}` or `{id, error}`.

use std::borrow::Cow;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

pub(crate) mod admin_config_reload;
pub(crate) mod admin_login_openai;
pub(crate) mod admin_status;
pub(crate) mod approval_respond;
pub(crate) mod conversation_history;
pub(crate) mod conversation_subscribe;
pub(crate) mod conversations_list;
pub(crate) mod hello;
pub(crate) mod input_respond;
pub(crate) mod lease_report;
pub(crate) mod models_list;
pub(crate) mod prompt_send;
pub(crate) mod pty_attach;
pub(crate) mod pty_resize;
pub(crate) mod pty_write;
pub(crate) mod turn_interrupt;
pub(crate) mod turn_steer;

/// An opaque position in a paged list. A client passes it back unchanged to get the
/// next page and never looks inside.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct PageCursor(String);

impl PageCursor {
    /// A cursor with the given contents. Only the daemon makes cursors.
    pub fn new(value: impl Into<String>) -> Self {
        PageCursor(value.into())
    }

    /// The contents, for the daemon that made the cursor.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What is wrong with `config.toml`, as `admin.status` and `admin.config_reload` report
/// it. The daemon keeps its old settings until the file is valid again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigFileError {
    /// What is wrong, in one sentence.
    pub message: String,
    /// The line of the error, counted from 1, when the error has a place in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The column of the error, counted from 1, when the error has a place in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The dotted path of the key that holds the error, such as `model.effort` or
    /// `permissions.rules[2].effect`, when the error belongs to one key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Raw bytes, such as PTY input and output, carried as a standard base64 string because
/// JSON strings cannot hold arbitrary bytes.
///
/// `Debug` shows only the length: PTY input can be a password typed at a `sudo` prompt.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Base64Bytes(Vec<u8>);

impl Base64Bytes {
    /// Wraps `bytes`.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Base64Bytes(bytes.into())
    }

    /// The bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The bytes, by value.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for Base64Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Base64Bytes(<{} bytes>)", self.0.len())
    }
}

impl Serialize for Base64Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(Base64Visitor)
    }
}

struct Base64Visitor;

impl Visitor<'_> for Base64Visitor {
    type Value = Base64Bytes;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a standard base64 string")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Base64Bytes, E> {
        STANDARD.decode(text).map(Base64Bytes).map_err(E::custom)
    }
}

impl JsonSchema for Base64Bytes {
    fn schema_name() -> Cow<'static, str> {
        "Base64Bytes".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "contentEncoding": "base64" })
    }
}

#[cfg(test)]
mod tests;
