//! Finding the daemon: `daemon.json` next to the socket, or the default socket path.
//!
//! The daemon writes `$XDG_RUNTIME_DIR/efr/daemon.json` atomically once its socket is
//! open. It is a hint, not the truth (the daemon's lock file is): a client reads it to
//! learn the socket path and to refuse a daemon of another protocol version before it
//! connects. Without the file the client tries the default socket path, which also
//! covers a daemon that is still starting.
//!
//! | `daemon.json` | Result |
//! |---|---|
//! | missing | the default socket path, no daemon info |
//! | same protocol, absolute socket path | that socket path and the info |
//! | another protocol | `ProtocolMismatch`, without connecting |
//! | relative socket path | `RelativeSocket` |
//! | unreadable or not valid JSON | `ReadDaemonJson` or `InvalidDaemonJson` |

use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::{DaemonId, PROTOCOL_VERSION};
use efr_stdx::paths::Dirs;
use serde::{Deserialize, Serialize};

use crate::ClientError;

/// The contents of `daemon.json`: `{pid, socket, protocol, daemon_id, tailnet_endpoint?}`.
///
/// `efr-daemon/src/discovery.rs` writes this shape; unknown members are ignored so a
/// newer daemon can add some.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// The daemon's process id.
    pub pid: u32,
    /// The daemon's Unix socket.
    pub socket: PathBuf,
    /// The daemon's protocol version.
    pub protocol: u32,
    /// The daemon's identity, which hello reports too.
    pub daemon_id: DaemonId,
    /// The daemon's tailnet listener, once the phone milestone adds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet_endpoint: Option<String>,
}

/// Where to connect, as discovery found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The socket to connect to.
    pub socket: PathBuf,
    /// The daemon's `daemon.json`, when there was one.
    pub info: Option<DaemonInfo>,
}

/// Finds the daemon's socket under `dirs`, following the table in the module doc.
pub async fn discover(dirs: &Dirs) -> Result<Discovered, ClientError> {
    let json_path = dirs.daemon_json_path();
    let info = read_daemon_json(&json_path).await?;
    decide(dirs.socket_path(), &json_path, info)
}

/// Reads and parses `daemon.json` at `path`; `None` when there is no such file.
pub async fn read_daemon_json(path: &Path) -> Result<Option<DaemonInfo>, ClientError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ClientError::ReadDaemonJson { path: path.to_path_buf(), source });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| ClientError::InvalidDaemonJson { path: path.to_path_buf(), source })
}

/// The decision of [`discover`], without the file system.
fn decide(
    default_socket: PathBuf,
    json_path: &Path,
    info: Option<DaemonInfo>,
) -> Result<Discovered, ClientError> {
    let Some(info) = info else {
        return Ok(Discovered { socket: default_socket, info: None });
    };
    if info.protocol != PROTOCOL_VERSION {
        return Err(ClientError::ProtocolMismatch {
            daemon: info.protocol,
            client: PROTOCOL_VERSION,
        });
    }
    if !info.socket.is_absolute() {
        return Err(ClientError::RelativeSocket {
            path: json_path.to_path_buf(),
            socket: info.socket,
        });
    }
    Ok(Discovered { socket: info.socket.clone(), info: Some(info) })
}

#[cfg(test)]
mod tests;
