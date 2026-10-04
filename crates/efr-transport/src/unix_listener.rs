//! The Unix socket listener: mode 0600, the peer's uid checked on every connection.
//!
//! Two independent guards keep other users out. No other user can ever reach the socket
//! file while its mode is anything but 0600: `bind` creates it with the process umask,
//! so it is bound inside a fresh staging directory of mode 0700 next to its path,
//! restricted to 0600 there, and only then renamed into place. And every accepted
//! connection's uid, as the kernel reports it with `SO_PEERCRED`, must be the daemon's
//! own; any other peer is closed before it can send a frame.
//!
//! A socket file that no daemon answers on is stale (left by a crash) and is replaced.
//! A socket that answers belongs to a running daemon and is never taken over. Anything
//! that is not a socket is left alone and reported.

use std::fs::{self, DirBuilder, Permissions};
use std::future::Future;
use std::io;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_stdx::time::Clock;
use nix::sys::socket::{getsockopt, sockopt};
use tokio::net::UnixStream;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::connection::{self, ConnectionParams};
use crate::{ConnId, Dispatcher, PeerCred, TransportError};

/// The socket's mode: owner read and write, nothing for anyone else.
const SOCKET_MODE: u32 = 0o600;

/// The mode of a socket directory that the listener creates.
const DIR_MODE: u32 = 0o700;

/// How long the accept loop waits after a failed accept, such as when the process is
/// out of file descriptors, before it tries again.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// A connection that passed the uid check.
#[derive(Debug)]
#[non_exhaustive]
pub struct Accepted {
    /// The connected stream.
    pub stream: UnixStream,
    /// The peer's credentials, from the kernel.
    pub peer: PeerCred,
    /// The id given to the connection.
    pub conn_id: ConnId,
}

/// The daemon's listening Unix socket.
///
/// Dropping it removes the socket file, unless another listener has replaced the file
/// since, so a daemon that shuts down never deletes its successor's socket.
#[derive(Debug)]
pub struct UnixListener {
    inner: tokio::net::UnixListener,
    path: PathBuf,
    /// The device and inode of the socket file this listener created.
    identity: (u64, u64),
    allowed_uid: u32,
    next_conn: AtomicU64,
}

impl UnixListener {
    /// Creates the socket at `path` with mode 0600 and starts listening. Only processes
    /// of the daemon's own uid may connect.
    ///
    /// The parent directory is created with mode 0700 when it is missing. Fails when a
    /// daemon already answers at `path` or when something other than a socket is there.
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self, TransportError> {
        Self::bind_allowing(path.into(), nix::unistd::getuid().as_raw()).await
    }

    /// [`bind`](Self::bind), with the uid that may connect given explicitly.
    pub(crate) async fn bind_allowing(
        path: PathBuf,
        allowed_uid: u32,
    ) -> Result<Self, TransportError> {
        let dir = parent_dir(&path);
        if !dir.exists() {
            DirBuilder::new()
                .recursive(true)
                .mode(DIR_MODE)
                .create(dir)
                .map_err(|source| TransportError::CreateDir { path: dir.to_path_buf(), source })?;
        }
        check_existing(&path).await?;
        let staging = staging_dir(&path);
        let staged = bind_staged(&staging, &path);
        let cleaned = remove_staging(&staging, &path);
        let inner = staged?;
        if let Err(source) = cleaned {
            // The socket is in place; a staging directory left behind is removed by the
            // next bind, so this costs nothing but a stray name.
            tracing::debug!(error = %source, staging = %staging.display(), "could not remove the staging directory");
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| TransportError::Inspect { path: path.clone(), source })?;
        Ok(UnixListener {
            inner,
            path,
            identity: (metadata.dev(), metadata.ino()),
            allowed_uid,
            next_conn: AtomicU64::new(1),
        })
    }

    /// The socket's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The only uid that may connect.
    pub fn allowed_uid(&self) -> u32 {
        self.allowed_uid
    }

    /// Waits for the next connection and checks its peer's uid.
    ///
    /// A peer of another uid is closed at once and reported as
    /// [`TransportError::PeerRejected`]; the listener keeps working after any error.
    pub async fn accept(&self) -> Result<Accepted, TransportError> {
        let (stream, _) =
            self.inner.accept().await.map_err(|source| TransportError::Accept { source })?;
        let credentials = getsockopt(&stream, sockopt::PeerCredentials)
            .map_err(|source| TransportError::PeerCredentials { source })?;
        // A pid of 0 means the kernel could not name the peer's process in this pid
        // namespace.
        let pid = u32::try_from(credentials.pid()).ok().filter(|pid| *pid != 0);
        let peer = PeerCred::new(credentials.uid(), pid);
        authorize(peer, self.allowed_uid)?;
        let conn_id = ConnId::new(self.next_conn.fetch_add(1, Ordering::Relaxed));
        Ok(Accepted { stream, peer, conn_id })
    }

    /// Accepts connections and serves each one in its own task until `shutdown`
    /// completes. Then it cancels every request still in flight, waits for the
    /// connections to close, and removes the socket file.
    ///
    /// `clock` times the retry after a failed accept and each closing connection's flush.
    pub async fn serve<D: Dispatcher>(
        self,
        dispatcher: Arc<D>,
        clock: Arc<dyn Clock>,
        shutdown: impl Future<Output = ()> + Send,
    ) {
        let stopping = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut shutdown = std::pin::pin!(shutdown);
        tracing::info!(socket = %self.path.display(), "listening");
        loop {
            let accepted = tokio::select! {
                biased;
                () = &mut shutdown => break,
                Some(joined) = connections.join_next(), if !connections.is_empty() => {
                    report_panic(joined);
                    continue;
                }
                accepted = self.accept() => accepted,
            };
            match accepted {
                Ok(Accepted { stream, peer, conn_id }) => {
                    let params = ConnectionParams {
                        conn_id,
                        peer,
                        dispatcher: Arc::clone(&dispatcher),
                        clock: Arc::clone(&clock),
                        shutdown: stopping.child_token(),
                    };
                    connections.spawn(connection::run(stream, params).in_current_span());
                }
                Err(
                    error @ (TransportError::PeerRejected { .. }
                    | TransportError::PeerCredentials { .. }),
                ) => {
                    tracing::warn!(error = %error, "closed a connection");
                }
                Err(error) => {
                    tracing::warn!(error = %error, "accept failed; retrying");
                    tokio::select! {
                        biased;
                        () = &mut shutdown => break,
                        () = clock.sleep(ACCEPT_RETRY) => {}
                    }
                }
            }
        }
        stopping.cancel();
        while let Some(joined) = connections.join_next().await {
            report_panic(joined);
        }
        tracing::info!(socket = %self.path.display(), "stopped listening");
    }
}

/// Logs a connection task that panicked. The connection loop catches its handlers'
/// panics itself, so one that reaches here is a bug in the transport.
fn report_panic(joined: Result<(), JoinError>) {
    if let Err(error) = joined
        && error.is_panic()
    {
        tracing::error!("a connection task panicked");
    }
}

impl Drop for UnixListener {
    fn drop(&mut self) {
        let ours = fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity);
        if ours {
            // A leftover file is replaced by the next daemon, so a failure here is
            // harmless and has nowhere to go from a destructor.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Lets in only the allowed uid.
pub(crate) fn authorize(peer: PeerCred, allowed_uid: u32) -> Result<(), TransportError> {
    if peer.uid() == allowed_uid {
        Ok(())
    } else {
        Err(TransportError::PeerRejected { uid: peer.uid(), pid: peer.pid(), allowed: allowed_uid })
    }
}

/// Decides whether `path` is free to bind: nothing there, or a stale socket.
async fn check_existing(path: &Path) -> Result<(), TransportError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(TransportError::Inspect { path: path.to_path_buf(), source }),
    };
    if !metadata.file_type().is_socket() {
        return Err(TransportError::NotASocket { path: path.to_path_buf() });
    }
    match UnixStream::connect(path).await {
        Ok(_) => Err(TransportError::AddressInUse { path: path.to_path_buf() }),
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(TransportError::Inspect { path: path.to_path_buf(), source }),
    }
}

/// The directory that holds `path`; a bare file name lives in the current directory.
fn parent_dir(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// A hidden directory next to `path`, unique to this process, in which the socket is
/// bound before it is restricted and renamed into place. It is in the same directory as
/// `path`, so the rename never crosses a file system.
fn staging_dir(path: &Path) -> PathBuf {
    parent_dir(path).join(format!(".{}.{}.tmp", file_name(path), std::process::id()))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Binds a socket for `path` inside a new `staging` directory of mode 0700, restricts it
/// to 0600 and renames it to `path`. Until the rename, only the owner can reach it.
fn bind_staged(staging: &Path, path: &Path) -> Result<tokio::net::UnixListener, TransportError> {
    let (inner, temp) = stage(staging, path)?;
    fs::rename(&temp, path).map_err(|source| TransportError::Rename {
        from: temp,
        to: path.to_path_buf(),
        source,
    })?;
    Ok(inner)
}

/// The first half of [`bind_staged`]: the private directory and the restricted socket in
/// it, not yet renamed. Returns the listener and the socket's temporary path.
fn stage(
    staging: &Path,
    path: &Path,
) -> Result<(tokio::net::UnixListener, PathBuf), TransportError> {
    remove_staging(staging, path)
        .map_err(|source| TransportError::Inspect { path: staging.to_path_buf(), source })?;
    let create_error = |source| TransportError::CreateDir { path: staging.to_path_buf(), source };
    // The umask can only take bits away from 0700, so the directory is never wider; the
    // mode is set again in case the umask took the owner's bits too.
    DirBuilder::new().mode(DIR_MODE).create(staging).map_err(create_error)?;
    fs::set_permissions(staging, Permissions::from_mode(DIR_MODE)).map_err(create_error)?;
    let temp = staging.join(file_name(path));
    let inner = tokio::net::UnixListener::bind(&temp)
        .map_err(|source| TransportError::Bind { path: temp.clone(), source })?;
    fs::set_permissions(&temp, Permissions::from_mode(SOCKET_MODE))
        .map_err(|source| TransportError::SetPermissions { path: temp.clone(), source })?;
    Ok((inner, temp))
}

/// Removes a staging directory and the socket in it, as a crash of a process with the
/// same pid may have left them. Anything else in it makes the removal fail, so a
/// directory that efr did not create is never emptied.
fn remove_staging(staging: &Path, path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(staging) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_dir() => return fs::remove_file(staging),
        Ok(_) => {}
    }
    let socket = staging.join(file_name(path));
    match fs::symlink_metadata(&socket) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&socket)?,
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::remove_dir(staging)
}

#[cfg(test)]
mod tests;
