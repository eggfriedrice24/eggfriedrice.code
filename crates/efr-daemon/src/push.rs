//! Pushes to the paired phones through the Expo push service (ADR 0028): the rule of
//! when to push, the repeats, and the open tickets.

use efr_transport::ConnectionContext;

/// The push service of efrd.
#[derive(Debug)]
pub(crate) struct Push {}

impl Push {
    /// The push service that sends to `endpoint`, the base URL of the Expo push service
    /// (`EFR_EXPO_PUSH_URL`, else `https://exp.host`).
    pub(crate) fn new(_endpoint: Option<String>) -> Self {
        Push {}
    }

    /// The capability `push` of the connection of `context`, or `None` when this efrd
    /// takes no push token there.
    pub(crate) fn capability(&self, _context: &ConnectionContext) -> Option<bool> {
        None
    }
}
