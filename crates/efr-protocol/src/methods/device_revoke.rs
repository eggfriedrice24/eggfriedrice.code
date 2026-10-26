//! `device.revoke`: take a device's right to connect away, for `efr device revoke`.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::DeviceId;

/// The params of `device.revoke`, an admin method (Unix socket only).
///
/// The daemon marks the device revoked and closes its open connections at once. A
/// device that does not exist is `not_found`. A revoke of a revoked device changes
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRevoke {
    /// The device.
    pub device_id: DeviceId,
}

/// The result of `device.revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRevokeResult {
    /// When the device was revoked: now, or the time of the first revoke.
    pub revoked_at: Timestamp,
}
