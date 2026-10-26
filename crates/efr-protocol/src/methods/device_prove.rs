//! `device.prove`: a remote device proves that it holds its enrolled key.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Base64Bytes, DeviceId, ScopeName};

/// The params of `device.prove`, a remote method that runs before the proof.
///
/// The device signs [`ProveMessage::to_bytes`](crate::ProveMessage::to_bytes) of the
/// `hello` result's `challenge`, `daemon_id`, `protocol` and `prove` facts. Before this
/// method succeeds, a remote connection may call only `hello`, `device.enroll` and
/// `device.prove`. A wrong signature, or a device that is revoked or that enrolled from
/// another node, is `unauthorized`; the daemon closes the connection after a few
/// failed proofs. The Unix socket refuses the method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceProve {
    /// The device. When the `hello` sent a `device_id`, it is the same.
    pub device_id: DeviceId,
    /// The ECDSA P-256 signature with SHA-256 of the message, in DER, as standard
    /// base64.
    pub signature: Base64Bytes,
}

/// The result of `device.prove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceProveResult {
    /// What the connection may do now.
    pub scopes: Vec<ScopeName>,
}
