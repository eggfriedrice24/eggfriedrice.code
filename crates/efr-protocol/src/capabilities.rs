//! Optional features that each side of a connection announces in `hello`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Optional features that one side of a connection supports, announced in `hello`.
///
/// Every key is optional, so a new key is an additive change: an old peer ignores it,
/// and a new peer reads its absence as "not supported". Keys that this build does not
/// know are kept in [`extra`](Self::extra), so a proxy can pass them on unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    /// The daemon accepts the admin methods (`admin.*`) on this connection. Only the
    /// Unix socket ever offers them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    /// The daemon starts `pty.attach` with a screen snapshot; from a client, the client
    /// can render one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_snapshots: Option<bool>,
    /// Keys that this build does not know, sorted by name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Capabilities {
    /// True when no key is set, known or extra.
    pub fn is_empty(&self) -> bool {
        self.admin.is_none() && self.screen_snapshots.is_none() && self.extra.is_empty()
    }
}

#[cfg(test)]
mod tests;
