//! `admin.sandbox_check`: run the sandbox probe now, for `efr sandbox check`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{SandboxCheck, SandboxStatus};

/// The params of `admin.sandbox_check`, an admin method (Unix socket only). It takes
/// none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminSandboxCheck {}

/// The result of `admin.sandbox_check`: the new status and every check behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminSandboxCheckResult {
    /// The status after this probe; the daemon uses it from now on.
    pub status: SandboxStatus,
    /// Each check with its outcome and, for a failure, its fix.
    pub checks: Vec<SandboxCheck>,
    /// The cost of one launch (bubblewrap and Landlock, no shell), in microseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_us: Option<u64>,
    /// The cost of one launch with the user's shell snapshot, in microseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_launch_us: Option<u64>,
}
