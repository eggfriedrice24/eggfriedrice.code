//! `device.list`: the enrolled devices, for `efr device list`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::DeviceInfo;

/// The params of `device.list`, an admin method (Unix socket only). It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceList {}

/// The result of `device.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceListResult {
    /// Every enrolled device, the revoked ones too, the oldest first.
    pub devices: Vec<DeviceInfo>,
}
