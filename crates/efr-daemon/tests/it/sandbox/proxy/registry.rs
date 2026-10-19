//! A local HTTPS server for the proxy tests: a blocking rustls server on a loopback
//! address of its own (`127.0.0.2:0` and up), with a leaf of a test CA that `openssl`
//! makes at test time below the test's dirs (never /tmp), one std thread per
//! connection, an answer per request from the test, and a log of each request. The
//! server answers one request per connection (`Connection: close`).
//!
//! The daemon's proxy reaches it through the table of `TestDaemon::proxy_hosts`: the
//! test name resolves to the server's own loopback address, and port 443 of that address
//! reaches the server's real port.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The largest request head and body that the server reads.
const MAX_BYTES: usize = 1 << 20;

/// A test CA and one leaf for the test names, as files below `dir`.
pub(super) struct TestCa {
    /// The CA's certificate in PEM, for curl, cargo and git.
    pub(super) pem: PathBuf,
    cert: Vec<u8>,
    key: Vec<u8>,
}

impl TestCa {
    /// Makes a CA and a leaf for `names` with `openssl` in `dir`.
    pub(super) async fn new(dir: &Path, names: &[&str]) -> TestCa {
        std::fs::create_dir_all(dir).unwrap();
        let san: Vec<String> = names.iter().map(|name| format!("DNS:{name}")).collect();
        std::fs::write(
            dir.join("leaf.ext"),
            format!(
                "subjectAltName={}\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature\n\
                 extendedKeyUsage=serverAuth\n",
                san.join(",")
            ),
        )
        .unwrap();
        let ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"];
        let steps: [Vec<&str>; 5] = [
            [&["req", "-x509"][..], &ec[..], &["-keyout", "ca.key", "-out", "ca.pem"][..]].concat(),
            [&["req"][..], &ec[..], &["-keyout", "leaf.key", "-out", "leaf.csr"][..]].concat(),
            vec![
                "x509",
                "-req",
                "-in",
                "leaf.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-days",
                "2",
                "-extfile",
                "leaf.ext",
                "-out",
                "leaf.pem",
            ],
            vec!["x509", "-in", "leaf.pem", "-outform", "DER", "-out", "leaf.der"],
            vec![
                "pkcs8", "-topk8", "-nocrypt", "-in", "leaf.key", "-outform", "DER", "-out",
                "leaf.p8",
            ],
        ];
        for (n, args) in steps.iter().enumerate() {
            let mut args = args.clone();
            match n {
                0 => args.extend([
                    "-days",
                    "2",
                    "-subj",
                    "/CN=efr test CA",
                    "-addext",
                    "basicConstraints=critical,CA:TRUE",
                    "-addext",
                    "keyUsage=critical,keyCertSign,cRLSign",
                ]),
                1 => args.extend(["-subj", "/CN=efr test leaf"]),
                _ => {}
            }
            let output = efr_stdx::process::command("openssl", dir)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "openssl {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        TestCa {
            pem: dir.join("ca.pem"),
            cert: std::fs::read(dir.join("leaf.der")).unwrap(),
            key: std::fs::read(dir.join("leaf.p8")).unwrap(),
        }
    }

    fn config(&self) -> Arc<rustls::ServerConfig> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone())),
            )
            .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(config)
    }
}

/// One request that the server read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Logged {
    pub(super) method: String,
    /// The target with its query.
    pub(super) path: String,
    /// The header names, in lower case.
    pub(super) headers: Vec<String>,
    /// The value of the `Host` header.
    pub(super) host: String,
    /// The body bytes that came.
    pub(super) body: usize,
}

impl Logged {
    /// True when the request carried credentials.
    pub(super) fn authorized(&self) -> bool {
        self.headers.iter().any(|name| name == "authorization")
    }
}

/// The answer to one request.
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) headers: Vec<(&'static str, String)>,
    pub(super) body: Vec<u8>,
}

impl Answer {
    /// `200 OK` with `body`.
    pub(super) fn ok(body: impl Into<Vec<u8>>) -> Answer {
        Answer { status: 200, headers: Vec::new(), body: body.into() }
    }

    /// `404 Not Found`.
    pub(super) fn not_found() -> Answer {
        Answer { status: 404, headers: Vec::new(), body: b"not found\n".to_vec() }
    }
}

type Route = dyn Fn(&Logged) -> Answer + Send + Sync;

/// A running server. Its threads end when the test process ends; they hold no file
/// of the test.
pub(super) struct Server {
    address: SocketAddr,
    log: Arc<Mutex<Vec<Logged>>>,
}

impl Server {
    /// Serves `route` on `ip:0` with the leaf of `ca`.
    pub(super) fn start(
        ip: &str,
        ca: &TestCa,
        route: impl Fn(&Logged) -> Answer + Send + Sync + 'static,
    ) -> Server {
        let listener = TcpListener::bind((ip, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let config = ca.config();
        let route: Arc<Route> = Arc::new(route);
        let shared = Arc::clone(&log);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (config, route, log) =
                    (Arc::clone(&config), Arc::clone(&route), Arc::clone(&shared));
                std::thread::spawn(move || serve(stream, &config, &*route, &log));
            }
        });
        Server { address, log }
    }

    /// The address that the server listens on.
    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }

    /// The requests so far, in the order they came.
    pub(super) fn log(&self) -> Vec<Logged> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Reads one request of `stream`, logs it and writes the answer of `route`.
fn serve(
    stream: TcpStream,
    config: &Arc<rustls::ServerConfig>,
    route: &Route,
    log: &Mutex<Vec<Logged>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(connection) = rustls::ServerConnection::new(Arc::clone(config)) else { return };
    let mut tls = rustls::StreamOwned::new(connection, stream);
    let Some(request) = read_request(&mut tls) else { return };
    log.lock().unwrap_or_else(PoisonError::into_inner).push(request.clone());
    let answer = route(&request);
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        answer.status,
        answer.body.len()
    );
    for (name, value) in &answer.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = tls.write_all(head.as_bytes());
    if request.method != "HEAD" {
        let _ = tls.write_all(&answer.body);
    }
    let _ = tls.flush();
    tls.conn.send_close_notify();
    let _ = tls.flush();
}

/// The head and the body of one HTTP/1.1 request; `None` when the client closed or sent
/// no request.
fn read_request(tls: &mut impl std::io::Read) -> Option<Logged> {
    let mut reader = BufReader::new(tls);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let (method, path) = (words.next()?.to_owned(), words.next()?.to_owned());
    let (mut headers, mut host, mut length, mut chunked, mut read) =
        (Vec::new(), String::new(), 0, false, line.len());
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).ok()?;
        read += n;
        if n == 0 || read > MAX_BYTES {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            "host" => host = value.to_owned(),
            "content-length" => length = value.parse().unwrap_or(0),
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            _ => {}
        }
        headers.push(name);
    }
    let body = if chunked { read_chunked(&mut reader) } else { read_body(&mut reader, length) };
    Some(Logged { method, path, headers, host, body })
}

/// Reads `length` body bytes, or what comes before the client stops; the count read.
fn read_body(reader: &mut impl std::io::Read, length: usize) -> usize {
    let mut body = vec![0; length.min(MAX_BYTES)];
    let mut got = 0;
    while got < body.len() {
        match reader.read(&mut body[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    got
}

/// Reads a chunked body; the count of its data bytes.
fn read_chunked(reader: &mut impl std::io::BufRead) -> usize {
    let mut total = 0;
    loop {
        let mut size = String::new();
        if reader.read_line(&mut size).unwrap_or(0) == 0 {
            return total;
        }
        let Ok(size) = usize::from_str_radix(size.trim().split(';').next().unwrap_or(""), 16)
        else {
            return total;
        };
        if size == 0 || total + size > MAX_BYTES {
            return total;
        }
        total += read_body(reader, size);
        let mut end = String::new();
        let _ = reader.read_line(&mut end);
    }
}
