//! The port forwards of a contained background job (DL1-DL16 of the v0.0.4 live items
//! plan): efrd's side of the forwarder that runs in the job (`efr-sbx forward`).
//!
//! A job gets forwards when it runs contained in `auto` with a network namespace of its
//! own (`network.mode = "none"` or the proxy, no grant of the whole network, no network
//! view) and `jobs.forward_ports` is true when it starts ([`plan_of`]). Then the spec's
//! job plan holds a [`ForwardPlan`], and efrd binds the job's forward socket
//! `$CALL/forward.sock` (mode 0600) before the launcher starts ([`bind`]); the launcher
//! binds it read-only into the job for the forwarder. The socket stays in the job's
//! entry of the job table until the job ends.

use std::path::{Path, PathBuf};

use efr_config::JobsSettings;
use efr_permissions::Egress;
use efr_protocol::{Grant, Launch};
use efr_sandbox::{FORWARD_SOCKET_FILE, ForwardPlan};

use crate::DaemonError;
use crate::sandbox::proxy::bind_socket;

/// The sentence of a job's start result when efr forwards its ports (DL16).
pub(crate) const START_NOTE: &str = "efr forwards each loopback port from 1024 up that this \
    job listens on to the same port of 127.0.0.1 on the user's machine; give the user the \
    URL, such as http://localhost:8765. A port in use there and well-known service ports \
    are not forwarded.";

/// The forward plan of a job that starts with `launch` in an efrd whose engine has
/// `egress`, with its forward socket at `socket` (`$CALL/forward.sock`); `None` when
/// the job gets no forwards (DL1, DL12).
///
/// Only a contained job with a network namespace of its own gets forwards: not a job in
/// the exit child, not one with a grant of the whole network, which listens on the
/// host's loopback already, and not a network view job, which cannot listen. The plan
/// says `proxy` exactly when the plan of the sandbox gives the job the network proxy
/// (`sandbox::plan::network`), so the spec check meets the same network.
pub(crate) fn plan_of(
    settings: &JobsSettings,
    launch: &Launch,
    egress: Egress,
    socket: PathBuf,
) -> Option<ForwardPlan> {
    if !settings.forward_ports {
        return None;
    }
    let Launch::Contained { grants } = launch else { return None };
    if grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork | Grant::NetView)) {
        return None;
    }
    Some(ForwardPlan::new(socket, egress == Egress::Proxy))
}

/// A job's forward socket, bound before its launcher starts. Dropping it closes the
/// socket.
#[derive(Debug)]
pub(crate) struct ForwardSocket {
    #[expect(dead_code, reason = "the forwards of a job are served in the next step")]
    listener: tokio::net::UnixListener,
    #[expect(dead_code, reason = "the forwards of a job are served in the next step")]
    plan: ForwardPlan,
}

/// Binds `$CALL/forward.sock` in the job's `call_dir` for the job's `plan`.
pub(crate) async fn bind(call_dir: &Path, plan: ForwardPlan) -> Result<ForwardSocket, DaemonError> {
    let dir = call_dir.to_path_buf();
    let path = call_dir.join(FORWARD_SOCKET_FILE);
    let listener = tokio::task::spawn_blocking(move || bind_socket(&dir, FORWARD_SOCKET_FILE))
        .await
        .map_err(|_| DaemonError::TaskPanicked { task: "forward socket" })?
        .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    let listener = tokio::net::UnixListener::from_std(listener)
        .map_err(|source| DaemonError::Io { path, source })?;
    Ok(ForwardSocket { listener, plan })
}

#[cfg(test)]
mod tests;
