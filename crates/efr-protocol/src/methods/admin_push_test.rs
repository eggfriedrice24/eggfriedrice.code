//! `admin.push_test`: send a test notice to each phone that has push on, for
//! `efr push test`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The params of `admin.push_test`, an admin method (Unix socket only). It takes none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminPushTest {}

/// The result of `admin.push_test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminPushTestResult {
    /// One row for each device with a push token, the oldest first. Empty when no
    /// paired phone has push on.
    pub devices: Vec<PushTestRow>,
}

/// What became of the test notice of one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushTestRow {
    /// The name of the device, such as `Anna's iPhone`.
    pub name: String,
    /// What became of the notice.
    pub result: PushTestResult,
    /// The code of the Expo push service for a notice that was `not_sent`, such as
    /// `DeviceNotRegistered`. Absent when the service gave no answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// The code of the Expo push service for a token that Apple or Expo no longer knows:
/// push is off for its device.
pub const PUSH_DEVICE_NOT_REGISTERED: &str = "DeviceNotRegistered";

/// The code of the Expo push service for a project with no valid push key.
pub const PUSH_INVALID_CREDENTIALS: &str = "InvalidCredentials";

/// The code of the Expo push service for a send that held tokens of two projects.
pub const PUSH_TOO_MANY_EXPERIENCE_IDS: &str = "PUSH_TOO_MANY_EXPERIENCE_IDS";

/// The code of the Expo push service for an account that asks for an access token.
pub const PUSH_UNAUTHORIZED: &str = "UNAUTHORIZED";

/// The codes of the Expo push service for too many notices in a short time.
pub const PUSH_RATE_LIMITED: [&str; 2] = ["MessageRateExceeded", "TOO_MANY_REQUESTS"];

/// What became of a test notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PushTestResult {
    /// Apple took the notice: its receipt is `ok`.
    Sent,
    /// The Expo push service took the notice, and no receipt came in time.
    NoReceipt,
    /// The notice did not go out; `code` says why.
    NotSent,
    /// A result that this build does not know.
    #[serde(other)]
    Unknown,
}

impl PushTestResult {
    /// Every result, in the order of the enum.
    pub const ALL: [PushTestResult; 4] = [
        PushTestResult::Sent,
        PushTestResult::NoReceipt,
        PushTestResult::NotSent,
        PushTestResult::Unknown,
    ];
}
