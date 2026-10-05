//! Connecting to the daemon's Unix socket.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::paths::MAX_SOCKET_PATH;
use efr_stdx::time::Clock;
use tokio::net::UnixStream;

use crate::ClientError;

/// Connects to the socket at `socket`, giving up after `timeout` on `clock`.
///
/// A missing socket and a refused connection both mean that no daemon is running, which
/// the CLI reports differently from other failures. A socket path too long for a socket
/// address is refused before connecting, with an error that names the limit.
pub(crate) async fn connect(
    socket: &Path,
    clock: &Arc<dyn Clock>,
    timeout: Duration,
) -> Result<UnixStream, ClientError> {
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(ClientError::SocketPathTooLong { socket: socket.to_path_buf() });
    }
    match clock.timeout(timeout, UnixStream::connect(socket)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(source)) => Err(classify(socket, source)),
        Err(_) => {
            Err(ClientError::ConnectTimedOut { socket: socket.to_path_buf(), after: timeout })
        }
    }
}

/// The error for a failed connect.
fn classify(socket: &Path, source: io::Error) -> ClientError {
    match source.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            ClientError::DaemonNotRunning { socket: socket.to_path_buf() }
        }
        _ => ClientError::Connect { socket: socket.to_path_buf(), source },
    }
}

#[cfg(test)]
mod tests;
