//! `lease.report`: tell the daemon what the client is watching.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ConversationId, PtyId};

/// The params of `lease.report`. A client reports a lease about every 25 seconds,
/// naming what it wants kept warm, and the daemon uses the leases to stop rendering
/// unwatched screens and to decide whether to notify. A new report replaces the
/// connection's previous one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseReport {
    /// The conversations the client shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<ConversationId>,
    /// The PTYs whose screens the client shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ptys: Vec<PtyId>,
    /// True when the client is in the foreground and a person can see it.
    pub visible: bool,
}

/// The result of `lease.report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseReportResult {
    /// How long the lease lasts without another report, in seconds.
    pub ttl_secs: u32,
}
