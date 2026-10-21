//! [`FakeForwarder`]: the forwarder of a job's port forwards (`efr-sbx forward`) as a
//! test plays it, for the tests of efrd's side with the [`FakeLauncher`](crate::FakeLauncher),
//! which starts no forwarder.
//!
//! A test reads the job's forward socket from the spec that the fake launcher kept
//! (`job.forward.socket`) and connects with [`FakeForwarder::connect`], which sends
//! `Hello`. It reports a listen with [`FakeForwarder::listen`], naming the server that
//! stands for the job's port: an [`echo_server`] of the test on a free loopback port. On
//! each `Open` of efrd, the fake connects to that server, opens a data connection to the
//! socket, sends `Data` and copies bytes both ways; for a port with no server it answers
//! `Data` with status 111 (`ECONNREFUSED`). [`FakeForwarder::send_raw`] sends any 28
//! bytes, such as a frame with a reserved byte that is not 0.
//!
//! The frames are encoded here by hand, as the layout of `efr_sandbox::Frame` says, so
//! the tests also check that efrd's decoder reads that layout.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// The length of every frame.
const FRAME_LEN: usize = 28;

const HELLO: u8 = 1;
const LISTEN: u8 = 2;
const GONE: u8 = 3;
const OPEN: u8 = 4;
const DATA: u8 = 5;

/// The status of a `Data` for a port with no server: `ECONNREFUSED`.
const REFUSED: u8 = 111;

/// The bytes of a frame: kind, family, status, 0, port (big-endian), 0, 0, id
/// (big-endian), the address in 16 bytes.
fn frame(kind: u8, status: u8, id: u32, addr: Option<IpAddr>, port: u16) -> [u8; FRAME_LEN] {
    let mut bytes = [0u8; FRAME_LEN];
    bytes[0] = kind;
    bytes[1] = 4;
    bytes[2] = status;
    bytes[4..6].copy_from_slice(&port.to_be_bytes());
    bytes[8..12].copy_from_slice(&id.to_be_bytes());
    match addr {
        Some(IpAddr::V4(ip)) => bytes[12..16].copy_from_slice(&ip.octets()),
        Some(IpAddr::V6(ip)) => {
            bytes[1] = 6;
            bytes[12..28].copy_from_slice(&ip.octets());
        }
        None => {}
    }
    bytes
}

/// A loopback server of the test on a free port that echoes every byte back, until the
/// test's runtime ends. Returns its address.
pub async fn echo_server() -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
                let _ = write.shutdown().await;
            });
        }
    });
    Ok(addr)
}

/// The forwarder of one job, as a test plays it. Dropping it closes its connections.
#[derive(Debug)]
pub struct FakeForwarder {
    socket: PathBuf,
    write: OwnedWriteHalf,
    servers: Arc<Mutex<BTreeMap<u16, SocketAddr>>>,
    closed: watch::Receiver<bool>,
    opens: watch::Receiver<u32>,
    task: JoinHandle<()>,
}

impl Drop for FakeForwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Connects to the socket `path`, whose name may be longer than a socket address
/// holds, through `/proc/self/fd/<dir>/<name>`, as efrd binds it.
pub async fn connect_socket(path: &Path) -> io::Result<UnixStream> {
    let dir = path.parent().ok_or_else(|| io::Error::other("the socket has no dir"))?;
    let name = path.file_name().ok_or_else(|| io::Error::other("the socket has no name"))?;
    let dir = File::open(dir)?;
    let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    let stream = UnixStream::connect(&short).await;
    drop(dir);
    stream
}

impl FakeForwarder {
    /// Connects to the job's forward socket `socket` and sends `Hello`.
    pub async fn connect(socket: &Path) -> io::Result<FakeForwarder> {
        let stream = connect_socket(socket).await?;
        let (mut read, mut write) = stream.into_split();
        write.write_all(&frame(HELLO, 1, 0, None, 0)).await?;
        let servers: Arc<Mutex<BTreeMap<u16, SocketAddr>>> = Arc::default();
        let (closed_tx, closed) = watch::channel(false);
        let (opens_tx, opens) = watch::channel(0u32);
        let socket_path = socket.to_path_buf();
        let known = Arc::clone(&servers);
        let task = tokio::spawn(async move {
            loop {
                let mut bytes = [0u8; FRAME_LEN];
                if read.read_exact(&mut bytes).await.is_err() {
                    let _ = closed_tx.send(true);
                    return;
                }
                if bytes[0] != OPEN {
                    continue;
                }
                let port = u16::from_be_bytes([bytes[4], bytes[5]]);
                let id = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
                let server =
                    known.lock().unwrap_or_else(PoisonError::into_inner).get(&port).copied();
                opens_tx.send_modify(|count| *count += 1);
                tokio::spawn(answer(socket_path.clone(), id, server));
            }
        });
        Ok(FakeForwarder { socket: socket.to_path_buf(), write, servers, closed, opens, task })
    }

    /// The job's forward socket.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sends 28 bytes on the control connection as they are.
    pub async fn send_raw(&mut self, bytes: [u8; FRAME_LEN]) -> io::Result<()> {
        self.write.write_all(&bytes).await
    }

    /// Reports that the job listens on `addr:port`; `server` is the test's server that
    /// stands for it, which an `Open` of the port reaches.
    pub async fn listen(
        &mut self,
        addr: IpAddr,
        port: u16,
        server: Option<SocketAddr>,
    ) -> io::Result<()> {
        if let Some(server) = server {
            self.servers.lock().unwrap_or_else(PoisonError::into_inner).insert(port, server);
        }
        self.send_raw(frame(LISTEN, 0, 0, Some(addr), port)).await
    }

    /// Reports that the job no longer listens on `addr:port`.
    pub async fn gone(&mut self, addr: IpAddr, port: u16) -> io::Result<()> {
        self.send_raw(frame(GONE, 0, 0, Some(addr), port)).await
    }

    /// Waits until efrd closed the control connection.
    pub async fn closed(&mut self) {
        let _ = self.closed.wait_for(|closed| *closed).await;
    }

    /// The `Open` frames that efrd sent so far.
    pub fn opens(&self) -> u32 {
        *self.opens.borrow()
    }
}

/// Answers the `Open` of `id`: a data connection with `Data`, then the copy to `server`.
async fn answer(socket: PathBuf, id: u32, server: Option<SocketAddr>) {
    let target = match server {
        Some(server) => TcpStream::connect(server).await.ok(),
        None => None,
    };
    let Ok(mut data) = connect_socket(&socket).await else { return };
    let status = if target.is_some() { 0 } else { REFUSED };
    if data.write_all(&frame(DATA, status, id, None, 0)).await.is_err() {
        return;
    }
    if let Some(mut target) = target {
        let _ = tokio::io::copy_bidirectional(&mut data, &mut target).await;
    }
}
