//! [`FakeReach`]: the reach helper of a contained call (`efr-sbx reach`) as a test plays
//! it, for the tests of efrd's side with the [`FakeLauncher`](crate::FakeLauncher), which
//! starts no helper.
//!
//! A test reads the call's reach socket from the spec that the fake launcher kept
//! (`reach.socket`) and connects with [`FakeReach::connect`], which sends `Hello`. The
//! fake keeps each `Reach` and `Unreach` that efrd sends ([`FakeReach::told`]) and
//! answers nothing; a test sends a `Bound` with [`FakeReach::bound`]. A process of the
//! call that connects to a port is [`FakeReach::open`]: a data connection to the socket
//! that sends `Connect` and then carries the bytes. The fake binds no TCP port.
//!
//! The frames are encoded here by hand, as the layout of `efr_sandbox::Frame` says, so
//! the tests also check that efrd's decoder reads that layout.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::fake_forwarder::connect_socket;

/// The length of every frame.
const FRAME_LEN: usize = 28;

const HELLO: u8 = 1;
const REACH: u8 = 6;
const UNREACH: u8 = 7;
const BOUND: u8 = 8;
const CONNECT: u8 = 9;

/// The version of the reach helper's `Hello`.
const REACH_VERSION: u8 = 1;

/// The bytes of a frame: kind, family, status, 0, port (big-endian), 0, 0, id 0, the
/// address in 16 bytes.
fn frame(kind: u8, status: u8, addr: Option<IpAddr>, port: u16) -> [u8; FRAME_LEN] {
    let mut bytes = [0u8; FRAME_LEN];
    bytes[0] = kind;
    bytes[1] = 4;
    bytes[2] = status;
    bytes[4..6].copy_from_slice(&port.to_be_bytes());
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

/// The address and the port of a frame that names them.
fn target(bytes: &[u8; FRAME_LEN]) -> SocketAddr {
    let port = u16::from_be_bytes([bytes[4], bytes[5]]);
    let addr = if bytes[1] == 6 {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&bytes[12..28]);
        IpAddr::from(octets)
    } else {
        IpAddr::from([bytes[12], bytes[13], bytes[14], bytes[15]])
    };
    SocketAddr::new(addr, port)
}

/// One frame that efrd sent on the control connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    /// `Reach`: listen on this address and port.
    Reach(SocketAddr),
    /// `Unreach`: close the listener of this address and port.
    Unreach(SocketAddr),
    /// A frame of another kind, which efrd must never send.
    Other(u8),
}

/// The reach helper of one call, as a test plays it. Dropping it closes its control
/// connection.
#[derive(Debug)]
pub struct FakeReach {
    socket: PathBuf,
    write: OwnedWriteHalf,
    told: watch::Receiver<Vec<Told>>,
    closed: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

impl Drop for FakeReach {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeReach {
    /// Connects to the call's reach socket `socket` and sends `Hello`: the control
    /// connection.
    pub async fn connect(socket: &Path) -> io::Result<FakeReach> {
        let stream = connect_socket(socket).await?;
        let (mut read, mut write) = stream.into_split();
        write.write_all(&frame(HELLO, REACH_VERSION, None, 0)).await?;
        let (told_tx, told) = watch::channel(Vec::new());
        let (closed_tx, closed) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                let mut bytes = [0u8; FRAME_LEN];
                if read.read_exact(&mut bytes).await.is_err() {
                    let _ = closed_tx.send(true);
                    return;
                }
                let told = match bytes[0] {
                    REACH => Told::Reach(target(&bytes)),
                    UNREACH => Told::Unreach(target(&bytes)),
                    other => Told::Other(other),
                };
                told_tx.send_modify(|all| all.push(told));
            }
        });
        Ok(FakeReach { socket: socket.to_path_buf(), write, told, closed, task })
    }

    /// The call's reach socket.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sends 28 bytes on the control connection as they are.
    pub async fn send_raw(&mut self, bytes: [u8; FRAME_LEN]) -> io::Result<()> {
        self.write.write_all(&bytes).await
    }

    /// Answers a `Reach` of `addr:port` with a `Bound` of `status`: 0 when the helper
    /// listens, else the errno of its bind.
    pub async fn bound(&mut self, addr: IpAddr, port: u16, status: u8) -> io::Result<()> {
        self.send_raw(frame(BOUND, status, Some(addr), port)).await
    }

    /// Every `Reach` and `Unreach` that efrd sent so far, in order.
    pub fn told(&self) -> Vec<Told> {
        self.told.borrow().clone()
    }

    /// Waits until efrd sent at least `count` frames, and returns them all.
    pub async fn told_at_least(&mut self, count: usize) -> Vec<Told> {
        let _ = self.told.wait_for(|told| told.len() >= count).await;
        self.told()
    }

    /// Waits until efrd closed the control connection.
    pub async fn closed(&mut self) {
        let _ = self.closed.wait_for(|closed| *closed).await;
    }

    /// A process of the call connected to `127.0.0.1:port`: a data connection to the
    /// reach socket that sent `Connect`. The bytes of the call's connection go on it.
    pub async fn open(&self, port: u16) -> io::Result<UnixStream> {
        self.open_at(IpAddr::V4(Ipv4Addr::LOCALHOST), port).await
    }

    /// [`open`](Self::open) to `addr:port`.
    pub async fn open_at(&self, addr: IpAddr, port: u16) -> io::Result<UnixStream> {
        let mut stream = connect_socket(&self.socket).await?;
        stream.write_all(&frame(CONNECT, 0, Some(addr), port)).await?;
        Ok(stream)
    }
}
