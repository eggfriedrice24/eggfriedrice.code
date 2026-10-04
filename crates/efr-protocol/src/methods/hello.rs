//! `hello`: the first request on every connection.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Capabilities, DaemonId, DeviceId, Origin};

/// The params of `hello`, the first request on every connection.
///
/// The daemon answers a `protocol` other than its own with `protocol_mismatch` before any
/// other method runs, and it records the rest as the connection's context for logs and
/// for the origin of the prompts that the connection sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// The client's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
    pub protocol: u32,
    /// The kind of surface the client is. A connection on the tailnet listener is always
    /// treated as [`Origin::Phone`], whatever it claims.
    pub origin: Origin,
    /// The client program and its version, such as `efr 0.1.0`, for logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// What the client supports.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// The client's terminal, when it runs in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    /// The client's process id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The enrolled device that is connecting. Only remote connections send it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<DeviceId>,
}

/// The result of `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloResult {
    /// The daemon's identity. Clients pin it and warn when it changes.
    pub daemon_id: DaemonId,
    /// The daemon's [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
    pub protocol: u32,
    /// The daemon's build version, such as `0.1.0`.
    pub version: String,
    /// What the daemon offers on this connection.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Where the daemon keeps its files.
    pub paths: DaemonPaths,
    /// A fresh random nonce for this connection, as unpadded base64url. A remote device
    /// proves its key by signing it together with `daemon_id` and `protocol`; the Unix
    /// socket ignores it.
    pub challenge: String,
}

/// Directories of the daemon that clients show to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonPaths {
    /// The parent of every conversation's `$SCRATCH` directory.
    pub scratch_root: PathBuf,
    /// The daemon's data directory: the database, recordings and secrets.
    pub data_dir: PathBuf,
}
