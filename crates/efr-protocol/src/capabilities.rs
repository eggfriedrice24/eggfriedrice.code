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
    /// The daemon answers `conversations.search` and `conversation.search`. A client
    /// reads its absence as a daemon from before the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<bool>,
    /// The daemon merges the drafts of each part that a slow subscriber could not take
    /// yet, and sends the merged draft when the subscriber has room, so a client gets
    /// fewer drafts on a slow link but no gaps. Absent: a draft can be lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drafts_coalesced: Option<bool>,
    /// The daemon fills `live` in the snapshot of `conversation.subscribe`: the text
    /// and the reasoning of the running turn so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_live: Option<bool>,
    /// The daemon sets `local_only` in `approval_requested`, so a client knows which
    /// questions only the user's own machine may answer. Absent: a client cannot tell,
    /// and the daemon refuses an answer that it may not give.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_local_only: Option<bool>,
    /// The daemon takes `device.enroll` and `device.prove` on this connection. Only a
    /// connection of the tailnet listener offers them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_prove: Option<bool>,
    /// The daemon answers `conversations.watch`, so a client can follow the lists of
    /// the machine instead of asking `conversations.recent` again and again. Absent: a
    /// daemon from before the watch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversations_watch: Option<bool>,
    /// Keys that this build does not know, sorted by name.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Capabilities {
    /// True when no key is set, known or extra.
    pub fn is_empty(&self) -> bool {
        self.admin.is_none()
            && self.screen_snapshots.is_none()
            && self.search.is_none()
            && self.drafts_coalesced.is_none()
            && self.snapshot_live.is_none()
            && self.approval_local_only.is_none()
            && self.device_prove.is_none()
            && self.conversations_watch.is_none()
            && self.extra.is_empty()
    }
}

#[cfg(test)]
mod tests;
