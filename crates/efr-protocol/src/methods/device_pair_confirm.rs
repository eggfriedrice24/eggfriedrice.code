//! `device.pair_confirm`: the terminal's answer about a phone that wants to pair.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DeviceId, PairingId};

/// The params of `device.pair_confirm`, an admin method (Unix socket only). It answers
/// the `pending` item of a `device.pair_start` stream.
///
/// A pairing that is not pending, or that ended, is `not_found`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePairConfirm {
    /// The pairing.
    pub pairing_id: PairingId,
    /// True to enroll the device, false to refuse it.
    pub accept: bool,
}

/// The result of `device.pair_confirm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePairConfirmResult {
    /// The new device; absent when the answer was no.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
}
