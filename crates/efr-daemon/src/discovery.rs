//! How clients find the daemon: `daemon.json` next to the socket, and the daemon's
//! identity.
//!
//! `daemon.json` is `{pid, socket, protocol, daemon_id, tailnet_endpoint?}`, the shape
//! `efr_client::DaemonInfo` reads. It is written atomically once the socket is open
//! and removed at shutdown, but it is a hint: the lock file decides which daemon runs.
//!
//! The daemon id is minted once, at the first start, and kept in the data directory,
//! so clients that pin it notice when a socket path leads to another installation.

use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use efr_protocol::DaemonId;
use efr_stdx::id::uuid_v7;
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use serde::{Deserialize, Serialize};

use crate::DaemonError;

/// The file in the data directory that holds the daemon id.
pub(crate) const DAEMON_ID_FILE: &str = "daemon_id";

/// The contents of `daemon.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DaemonInfo {
    pub(crate) pid: u32,
    pub(crate) socket: PathBuf,
    pub(crate) protocol: u32,
    pub(crate) daemon_id: DaemonId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tailnet_endpoint: Option<String>,
}

/// Writes `info` to `path` atomically, with mode 0600.
pub(crate) fn write(path: &Path, info: &DaemonInfo) -> Result<(), DaemonError> {
    let mut json = serde_json::to_vec_pretty(info)
        .map_err(|source| DaemonError::EncodeDiscovery { source })?;
    json.push(b'\n');
    efr_stdx::fs::write_atomic(path, &json).map_err(|source| DaemonError::WriteFile { source })
}

/// Removes `path` when it still names this process, so a daemon that stops never
/// deletes the file of the daemon that replaced it.
pub(crate) fn remove(path: &Path, pid: u32) {
    let ours = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DaemonInfo>(&bytes).ok())
        .is_some_and(|info| info.pid == pid);
    if ours && let Err(error) = std::fs::remove_file(path) {
        tracing::debug!(error = %error, path = %path.display(), "could not remove daemon.json");
    }
}

/// The daemon id kept in `data_dir`, minted from `clock` and `rng` and saved when there
/// is none yet.
pub(crate) fn daemon_id(
    data_dir: &Path,
    clock: &dyn Clock,
    rng: &dyn Rng,
) -> Result<DaemonId, DaemonError> {
    let path = data_dir.join(DAEMON_ID_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            DaemonId::from_str(text.trim()).map_err(|_| DaemonError::InvalidDaemonId { path })
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            let id = DaemonId::from_uuid(uuid_v7(clock, rng));
            efr_stdx::fs::write_atomic(&path, format!("{id}\n").as_bytes())
                .map_err(|source| DaemonError::WriteFile { source })?;
            Ok(id)
        }
        Err(source) => Err(DaemonError::Io { path, source }),
    }
}

#[cfg(test)]
mod tests;
