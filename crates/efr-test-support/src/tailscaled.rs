//! A fake tailscaled for the tests of the remote listener: its LocalAPI on a Unix
//! socket in a temporary directory. Nothing reaches the real tailscaled.
//!
//! `status` answers the status that the test set (an error while it set none), `whois`
//! the identity that the test set (404 while it set none), and the watch of the IPN
//! bus sends the status as `InitialStatus`, then each line that the test pushes, and
//! ends when the test ends the watches. With no status the watch fails too, as a
//! tailscaled that is gone does.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

/// The version that the fake says it is.
pub(crate) const VERSION: &str = "1.104.1-t0123abcd-g4567ef89";

/// The tailnet address of the fake node.
pub(crate) const SELF_IP: &str = "100.101.102.103";

/// The stable id of the fake node.
pub(crate) const SELF_NODE: &str = "nSELFxxCNTRL";

/// The stable id of the fake tailnet.
pub(crate) const TAILNET: &str = "T1234CNTRL";

/// The user that owns the fake node and the phone.
pub(crate) const USER: i64 = 9_007_199_254_740_993;

/// The stable id of the phone's node.
pub(crate) const PHONE_NODE: &str = "nPHONExCNTRL";

/// A status of a running node with a TUN device, as tailscaled 1.104 sends it, cut to
/// the fields around the ones efrd reads.
pub(crate) fn status(ip: &str, tailnet: &str) -> Value {
    json!({
        "Version": VERSION,
        "TUN": true,
        "BackendState": "Running",
        "TailscaleIPs": [ip, "fd7a:115c:a1e0::1234:5678"],
        "Self": {
            "ID": SELF_NODE,
            "HostName": "desk",
            "DNSName": "desk.example.ts.net.",
            "UserID": USER,
            "Tags": null
        },
        "CurrentTailnet": {
            "Name": "user@example.com",
            "StableID": tailnet,
            "MagicDNSSuffix": "example.ts.net",
            "MagicDNSEnabled": true
        },
        "Peer": null
    })
}

/// A whois reply for the node `node` of the user `user`, with `tags`.
pub(crate) fn whois(node: &str, user: i64, tags: &[&str]) -> Value {
    json!({
        "Node": {
            "ID": 7,
            "StableID": node,
            "Name": "phone.example.ts.net.",
            "Tags": tags,
            "Hostinfo": { "Hostname": "phone", "ShareeNode": false }
        },
        "UserProfile": { "ID": user, "LoginName": "user@example.com", "DisplayName": "User" },
        "CapMap": {}
    })
}

#[derive(Debug, Default)]
struct Answers {
    status: Option<Value>,
    whois: Option<Value>,
    seen: Vec<String>,
}

/// A running fake tailscaled.
#[derive(Debug)]
pub(crate) struct FakeTailscaled {
    socket: PathBuf,
    answers: Arc<Mutex<Answers>>,
    /// A line for every open watch; `None` ends them.
    lines: broadcast::Sender<Option<String>>,
    _dir: tempfile::TempDir,
}

impl FakeTailscaled {
    /// Starts a fake whose status is [`status`] at [`SELF_IP`] and whose whois names
    /// the phone's node of [`USER`].
    pub(crate) fn start() -> FakeTailscaled {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tailscaled.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let answers = Arc::new(Mutex::new(Answers {
            status: Some(status(SELF_IP, TAILNET)),
            whois: Some(whois(PHONE_NODE, USER, &[])),
            seen: Vec::new(),
        }));
        let (lines, _) = broadcast::channel(16);
        let serving = (Arc::clone(&answers), lines.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (answers, lines) = (Arc::clone(&serving.0), serving.1.clone());
                tokio::spawn(answer(stream, answers, lines));
            }
        });
        FakeTailscaled { socket, answers, lines, _dir: dir }
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sets the status; `None` makes `status` fail.
    pub(crate) fn set_status(&self, status: Option<Value>) {
        self.lock().status = status;
    }

    /// Sets the whois answer; `None` makes `whois` answer 404.
    pub(crate) fn set_whois(&self, whois: Option<Value>) {
        self.lock().whois = whois;
    }

    /// Sends `line` on every open watch.
    pub(crate) fn push(&self, line: Value) {
        let _ = self.lines.send(Some(line.to_string()));
    }

    /// Ends every open watch.
    pub(crate) fn end_watches(&self) {
        let _ = self.lines.send(None);
    }

    /// The target of each request so far.
    pub(crate) fn seen(&self) -> Vec<String> {
        self.lock().seen.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Answers> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn answer(
    mut stream: UnixStream,
    answers: Arc<Mutex<Answers>>,
    lines: broadcast::Sender<Option<String>>,
) {
    let mut head = Vec::new();
    let mut buffer = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let Ok(read) = stream.read(&mut buffer).await else { return };
        if read == 0 {
            return;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let target = head.split(' ').nth(1).unwrap_or_default().to_owned();
    let (status, whois) = {
        let mut answers = answers.lock().unwrap_or_else(PoisonError::into_inner);
        answers.seen.push(target.clone());
        (answers.status.clone(), answers.whois.clone())
    };
    let path = target.split('?').next().unwrap_or_default();
    let reply = match path {
        "/localapi/v0/status" => match status {
            Some(status) => reply("200 OK", &status.to_string()),
            None => reply("503 Service Unavailable", "{}"),
        },
        "/localapi/v0/whois" => match whois {
            Some(whois) => reply("200 OK", &whois.to_string()),
            None => reply("404 Not Found", "no match for IP:port"),
        },
        "/localapi/v0/watch-ipn-bus" if status.is_none() => reply("503 Service Unavailable", "{}"),
        "/localapi/v0/watch-ipn-bus" => {
            let mut feed = lines.subscribe();
            let mut text = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntailscale-version: {VERSION}\r\nconnection: close\r\n\r\n"
            );
            if let Some(status) = status {
                text.push_str(
                    &json!({ "Version": VERSION, "State": 6, "InitialStatus": status }).to_string(),
                );
                text.push('\n');
            }
            if stream.write_all(text.as_bytes()).await.is_err() {
                return;
            }
            while let Ok(Some(line)) = feed.recv().await {
                if stream.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
            let _ = stream.shutdown().await;
            return;
        }
        _ => reply("404 Not Found", "not found"),
    };
    let _ = stream.write_all(&reply).await;
    let _ = stream.shutdown().await;
}

fn reply(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ntailscale-version: {VERSION}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}
