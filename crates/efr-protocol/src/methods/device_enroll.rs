//! `device.enroll`: a phone sends the token of a pairing QR code and its public key.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Base64Bytes, DeviceId, ScopeName, SecretText};

/// The params of `device.enroll`, a remote method that runs before the proof.
///
/// The daemon checks the token, shows the phone's facts at the terminal and answers
/// only after the terminal confirms. A wrong or used token is `unauthorized`; a no at
/// the terminal, or a pairing that expires, is `forbidden`. The phone checks
/// `daemon_id` and `protocol` of the `hello` result against the QR code before it
/// sends the token. The Unix socket refuses the method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceEnroll {
    /// The token of the QR code.
    pub token: SecretText,
    /// The name of the device, such as `Anna's iPhone`, for the terminal and for
    /// `device.list`.
    pub name: String,
    /// The device's P-256 public key: the 65-byte uncompressed point (`04`, then X and
    /// Y), as standard base64.
    pub public_key: Base64Bytes,
}

/// The result of `device.enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceEnrollResult {
    /// The new device. The phone keeps it and proves its key with `device.prove` on
    /// this connection and on each one after it.
    pub device_id: DeviceId,
    /// What the device may do once it proved its key.
    pub scopes: Vec<ScopeName>,
}
