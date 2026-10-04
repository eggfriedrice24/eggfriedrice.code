//! The protocol version.

/// The version of the wire protocol that this crate defines.
///
/// Both sides send it in `hello`, and the daemon answers a different value with
/// [`ErrorCode::ProtocolMismatch`](crate::ErrorCode::ProtocolMismatch) before any other
/// method runs. Additive changes keep the number: a new optional field, a new capability
/// key or a new event kind. Removing or re-typing a field bumps it, together with a new
/// `fixtures/vN/` directory and a line in the changelog of `docs/protocol.md`.
pub const PROTOCOL_VERSION: u32 = 1;
