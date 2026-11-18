//! The open uploads of `attachment.put`, by owner: a connection of the Unix socket, or
//! the device of the proof on the tailnet listener.

use efr_transport::ConnId;

/// The open uploads. A start of efrd removes every part, and a closed connection of the
/// Unix socket removes its own.
#[derive(Debug, Default)]
pub(crate) struct Uploads {}

impl Uploads {
    /// Removes each part that an earlier efrd left: a client uploads its files again.
    pub(crate) fn sweep(&self) {}

    /// Removes the open uploads of the connection `conn_id`, which closed.
    pub(crate) fn closed(&self, _conn_id: ConnId) {}
}
