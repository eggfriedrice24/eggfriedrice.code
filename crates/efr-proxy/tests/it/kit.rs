//! The test kit of the proxy (DP30): a table resolver, a connector that reaches only
//! loopback servers of the test, fixed own addresses, local upstream servers, a
//! client that speaks to a call's socket, and ClientHello bytes.
//!
//! No test reaches a real host or the real resolver. Names end in `.test`. The
//! constants `GLOBAL_*` are addresses outside both special-purpose registries: only
//! the table resolver hands them out, and the test connector refuses them, so no byte
//! leaves.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_permissions::{HostList, HostTable, MirrorHost};
use efr_protocol::CallId;
use efr_proxy::{
    CallPolicy, CallProxy, Connecting, Connector, Deps, Guard, Limits, OwnAddresses, Proxy,
    Resolver, Resolving,
};
use efr_test_support::Wait;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::watch;

pub(crate) const GLOBAL_V4: &str = "11.0.0.1";
pub(crate) const GLOBAL_V4_OWN: &str = "11.0.1.1";
pub(crate) const GLOBAL_V6: &str = "2a00:0:0:1::1";
pub(crate) const GLOBAL_V6_OWN: &str = "2a00:0:0:2::1";

pub(crate) fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// Names to addresses, and every name that the proxy asked for.
#[derive(Debug, Default)]
pub(crate) struct Table {
    names: Mutex<HashMap<String, Vec<Vec<IpAddr>>>>,
    asked: Mutex<Vec<String>>,
}

impl Table {
    /// `name` resolves to `addresses`.
    pub(crate) fn add(&self, name: &str, addresses: &[&str]) {
        self.answers(name, vec![addresses.iter().map(|address| ip(address)).collect()]);
    }

    /// `name` gives each answer in turn, and the last one after that.
    pub(crate) fn answers(&self, name: &str, answers: Vec<Vec<IpAddr>>) {
        self.names.lock().unwrap().insert(name.to_owned(), answers);
    }

    pub(crate) fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Resolver for Table {
    fn resolve<'a>(&'a self, host: &'a str) -> Resolving<'a> {
        self.asked.lock().unwrap().push(host.to_owned());
        let answer = {
            let mut names = self.names.lock().unwrap();
            names.get_mut(host).map(|answers| {
                if answers.len() > 1 { answers.remove(0) } else { answers[0].clone() }
            })
        };
        Box::pin(async move {
            answer.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such test name"))
        })
    }
}

/// A resolver that never answers.
#[derive(Debug, Default)]
pub(crate) struct Silent;

impl Resolver for Silent {
    fn resolve<'a>(&'a self, _host: &'a str) -> Resolving<'a> {
        Box::pin(std::future::pending())
    }
}

/// Reaches only loopback servers of the test: an address on port 443 or 80 goes to the
/// server's real port. Every other address is refused, so no byte leaves.
#[derive(Debug, Default)]
pub(crate) struct Loopback {
    routes: Mutex<HashMap<SocketAddr, SocketAddr>>,
    seen: Mutex<Vec<SocketAddr>>,
}

impl Loopback {
    pub(crate) fn route(&self, from: &str, to: SocketAddr) {
        self.routes.lock().unwrap().insert(from.parse().unwrap(), to);
    }

    pub(crate) fn seen(&self) -> Vec<SocketAddr> {
        self.seen.lock().unwrap().clone()
    }
}

impl Connector for Loopback {
    fn connect(&self, addr: SocketAddr) -> Connecting<'_> {
        self.seen.lock().unwrap().push(addr);
        let route = self.routes.lock().unwrap().get(&addr).copied();
        Box::pin(async move {
            if !addr.ip().is_loopback() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the test connector reaches only loopback",
                ));
            }
            let Some(to) = route.filter(|to| to.ip().is_loopback()) else {
                return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "no test server"));
            };
            TcpStream::connect(to).await
        })
    }
}

/// Fixed own addresses.
#[derive(Debug, Default)]
pub(crate) struct Own(pub(crate) Vec<(IpAddr, u8)>);

impl OwnAddresses for Own {
    fn addresses(&self) -> Vec<(IpAddr, u8)> {
        self.0.clone()
    }
}

/// A list from the config words, as efrd compiles it.
pub(crate) fn list(allow: &[&str], deny: &[&str], hosts: &[HostTable<'_>]) -> HostList {
    list_with(allow, deny, hosts, &[])
}

pub(crate) fn list_with(
    allow: &[&str],
    deny: &[&str],
    hosts: &[HostTable<'_>],
    mirrors: &[MirrorHost],
) -> HostList {
    let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>();
    HostList::compile(&words(allow), &words(deny), hosts, mirrors).unwrap()
}

/// Short times, so a timeout test ends quickly.
pub(crate) fn short() -> Limits {
    Limits {
        head_timeout: Duration::from_millis(200),
        hello_timeout: Duration::from_millis(200),
        resolve_timeout: Duration::from_millis(200),
        connect_timeout: Duration::from_millis(200),
        connect_total: Duration::from_millis(400),
        ..Limits::default()
    }
}

/// A proxy with the test seams, its socket dirs and the sender of its live list.
pub(crate) struct World {
    pub(crate) table: Arc<Table>,
    pub(crate) connector: Arc<Loopback>,
    pub(crate) proxy: Proxy,
    pub(crate) hosts: watch::Sender<Arc<HostList>>,
    dir: tempfile::TempDir,
    sockets: Mutex<u32>,
}

impl World {
    /// The loopback guard, no own address and the default limits.
    pub(crate) fn new(list: HostList) -> World {
        World::build(list, Guard::with_loopback(), Vec::new(), Limits::default())
    }

    pub(crate) fn build(
        list: HostList,
        guard: Guard,
        own: Vec<(IpAddr, u8)>,
        limits: Limits,
    ) -> World {
        World::with_resolver(list, guard, own, limits, None)
    }

    pub(crate) fn with_resolver(
        list: HostList,
        guard: Guard,
        own: Vec<(IpAddr, u8)>,
        limits: Limits,
        resolver: Option<Arc<dyn Resolver>>,
    ) -> World {
        let table = Arc::new(Table::default());
        let connector = Arc::new(Loopback::default());
        let deps = Deps {
            resolver: resolver.unwrap_or_else(|| Arc::clone(&table) as Arc<dyn Resolver>),
            connector: Arc::clone(&connector) as Arc<dyn Connector>,
            own: Arc::new(Own(own)),
            guard,
            limits,
        };
        let (hosts, receiver) = watch::channel(Arc::new(list));
        World {
            table,
            connector,
            proxy: Proxy::new(deps, receiver),
            hosts,
            dir: tempfile::tempdir().unwrap(),
            sockets: Mutex::new(0),
        }
    }

    /// Serves a call with these grants and this budget; gives the handle and its socket.
    pub(crate) fn call(&self, grants: &[(&str, u16)], budget: u64) -> (CallProxy, PathBuf) {
        self.call_with(policy(grants, &[], budget))
    }

    pub(crate) fn call_with(&self, policy: CallPolicy) -> (CallProxy, PathBuf) {
        let path = {
            let mut sockets = self.sockets.lock().unwrap();
            *sockets += 1;
            self.dir.path().join(format!("proxy-{sockets}.sock"))
        };
        let listener = UnixListener::bind(&path).unwrap();
        (self.proxy.serve(listener, policy), path)
    }
}

pub(crate) fn policy(grants: &[(&str, u16)], deny: &[&str], budget: u64) -> CallPolicy {
    CallPolicy {
        call: "0192f7a0-0000-7000-8000-000000000001".parse::<CallId>().unwrap(),
        grants: grants.iter().map(|(host, port)| ((*host).to_owned(), *port)).collect(),
        deny: deny.iter().map(|host| (*host).to_owned()).collect(),
        max_upload_bytes: budget,
    }
}

/// The default budget of a call, 4 MiB.
pub(crate) const BUDGET: u64 = 4 * 1024 * 1024;

/// A loopback server of the test: it echoes every byte, or answers each plain request
/// with `200 OK`, and keeps what each connection sent.
#[derive(Debug, Clone)]
pub(crate) struct Upstream {
    pub(crate) addr: SocketAddr,
    got: Arc<Mutex<Vec<Vec<u8>>>>,
    ended: Arc<Mutex<usize>>,
}

impl Upstream {
    /// A server that echoes each byte back.
    pub(crate) async fn echo() -> Upstream {
        Upstream::start(false).await
    }

    /// A server that reads one request head and answers `200 OK` with the body `ok`.
    pub(crate) async fn http() -> Upstream {
        Upstream::start(true).await
    }

    async fn start(http: bool) -> Upstream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(Mutex::new(0));
        let log = Arc::clone(&got);
        let ends = Arc::clone(&ended);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let log = Arc::clone(&log);
                let ends = Arc::clone(&ends);
                let index = {
                    let mut log = log.lock().unwrap();
                    log.push(Vec::new());
                    log.len() - 1
                };
                tokio::spawn(async move {
                    serve(&mut stream, &log, index, http).await;
                    *ends.lock().unwrap() += 1;
                });
            }
        });
        Upstream { addr, got, ended }
    }

    /// Waits until the server accepted `connections` connections and each of them
    /// ended, so the bytes of [`got`](Self::got) are all that it will get.
    pub(crate) async fn settled(&self, connections: usize) {
        Wait::new("the test server's connections to end")
            .until(|| {
                self.got.lock().unwrap().len() == connections
                    && *self.ended.lock().unwrap() == connections
            })
            .await
            .unwrap();
    }

    /// The bytes of each connection, in the order of the accepts.
    pub(crate) fn got(&self) -> Vec<Vec<u8>> {
        self.got.lock().unwrap().clone()
    }

    /// Every byte that reached the server.
    pub(crate) fn total(&self) -> usize {
        self.got().iter().map(Vec::len).sum()
    }
}

/// One connection of a test server: an echo, or one `200 OK` after the request head.
async fn serve(stream: &mut TcpStream, log: &Mutex<Vec<Vec<u8>>>, index: usize, http: bool) {
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let Ok(count) = stream.read(&mut chunk).await else { return };
        if count == 0 {
            return;
        }
        let head_done = {
            let mut log = log.lock().unwrap();
            log[index].extend_from_slice(&chunk[..count]);
            log[index].windows(4).any(|four| four == b"\r\n\r\n")
        };
        if http {
            if head_done {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
                return;
            }
        } else if stream.write_all(&chunk[..count]).await.is_err() {
            return;
        }
    }
}

/// A client of one call's socket.
pub(crate) struct Client {
    pub(crate) stream: UnixStream,
}

impl Client {
    pub(crate) async fn connect(path: &std::path::Path) -> Client {
        Client { stream: UnixStream::connect(path).await.unwrap() }
    }

    pub(crate) async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// Sends `CONNECT target` and gives the status code of the answer.
    pub(crate) async fn connect_to(&mut self, target: &str) -> u16 {
        self.send(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await;
        self.status().await
    }

    /// Reads one answer head and gives its status code.
    pub(crate) async fn status(&mut self) -> u16 {
        let head = self.head().await;
        head.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or(0)
    }

    /// Reads the bytes up to and with `CRLF CRLF`, one at a time, so no byte of a
    /// tunnel is taken.
    pub(crate) async fn head(&mut self) -> String {
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if self.stream.read(&mut byte).await.unwrap_or(0) == 0 {
                break;
            }
            head.push(byte[0]);
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    /// Everything until the proxy closes.
    pub(crate) async fn rest(&mut self) -> Vec<u8> {
        let mut rest = Vec::new();
        let _ = self.stream.read_to_end(&mut rest).await;
        rest
    }

    /// Reads exactly `count` bytes.
    pub(crate) async fn exact(&mut self, count: usize) -> Vec<u8> {
        let mut bytes = vec![0; count];
        self.stream.read_exact(&mut bytes).await.unwrap();
        bytes
    }

    /// True when the proxy closed the connection: a read gives EOF or an error.
    pub(crate) async fn closed(&mut self) -> bool {
        let mut byte = [0; 1];
        matches!(self.stream.read(&mut byte).await, Ok(0) | Err(_))
    }
}

/// The ClientHello that rustls writes for `name`. No server and no network: the bytes
/// go into a `Vec`.
pub(crate) fn rustls_hello(name: &str, sni: bool) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    config.enable_sni = sni;
    let server = rustls::pki_types::ServerName::try_from(name.to_owned()).unwrap();
    let mut connection = rustls::ClientConnection::new(Arc::new(config), server).unwrap();
    let mut bytes = Vec::new();
    while connection.wants_write() {
        connection.write_tls(&mut bytes).unwrap();
    }
    bytes
}

/// `hello` (one record, as rustls writes it) with one more extension of `kind` and
/// `size` zero bytes, in one record again.
pub(crate) fn with_extension(hello: &[u8], kind: u16, size: usize) -> Vec<u8> {
    let mut message = hello[5..].to_vec();
    // The extensions block: after the version, the random, the session id, the cipher
    // suites and the compression methods.
    let mut at = 4 + 2 + 32;
    at += 1 + usize::from(message[at]);
    at += 2 + usize::from(u16::from_be_bytes([message[at], message[at + 1]]));
    at += 1 + usize::from(message[at]);
    let block = usize::from(u16::from_be_bytes([message[at], message[at + 1]]));
    let mut extension = kind.to_be_bytes().to_vec();
    extension.extend_from_slice(&u16::try_from(size).unwrap().to_be_bytes());
    extension.resize(4 + size, 0);
    let end = at + 2 + block;
    message.splice(end..end, extension.iter().copied());
    let block = u16::try_from(block + extension.len()).unwrap().to_be_bytes();
    message[at..at + 2].copy_from_slice(&block);
    let body = u32::try_from(message.len() - 4).unwrap().to_be_bytes();
    message[1..4].copy_from_slice(&body[1..]);
    let mut bytes = vec![22, 3, 1];
    bytes.extend_from_slice(&u16::try_from(message.len()).unwrap().to_be_bytes());
    bytes.extend(message);
    bytes
}

/// One TLS application data record of `size` bytes.
pub(crate) fn record(size: usize) -> Vec<u8> {
    let mut bytes = vec![23, 3, 3];
    bytes.extend_from_slice(&u16::try_from(size).unwrap().to_be_bytes());
    bytes.resize(5 + size, 0x5a);
    bytes
}
