//! A fake tailscaled for the tests of efrd's remote listener: its LocalAPI on a Unix
//! socket in a temporary directory. Nothing reaches the real tailscaled.
//!
//! `status` answers the status that the test set (an error while it set none), `whois`
//! the identity that the test set (404 while it set none), and the watch of the IPN
//! bus sends the status as `InitialStatus`, then each line that the test pushes, and
//! ends when the test ends the watches. With no status the watch fails too, as a
//! tailscaled that is gone does.
//!
//! The fake does not look at the peer address of a whois: the identity that the test
//! set is the answer for every peer. So a test can connect from `127.0.0.1` and still
//! be any node of the tailnet.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::TestSupportError;

/// The version that the fake says it is.
pub const VERSION: &str = "1.104.1-t0123abcd-g4567ef89";

/// The tailnet address of the fake node.
pub const SELF_IP: &str = "100.101.102.103";

/// The stable id of the fake node.
pub const SELF_NODE: &str = "nSELFxxCNTRL";

/// The stable id of the fake tailnet.
pub const TAILNET: &str = "T1234CNTRL";

/// The user that owns the fake node and the phone. It is above 2^53, as an old
/// Tailscale id can be, so a reader that goes through a float fails.
pub const USER: i64 = 9_007_199_254_740_993;

/// The stable id of the phone's node.
pub const PHONE_NODE: &str = "nPHONExCNTRL";

/// The MagicDNS name of the phone's node, with the dot at its end.
pub const PHONE_NAME: &str = "phone.example.ts.net.";

/// The login name of [`USER`].
pub const LOGIN: &str = "user@example.com";

/// A status of a running node with a TUN device, as tailscaled 1.104 sends it, cut to
/// the fields around the ones efrd reads.
pub fn status(ip: &str, tailnet: &str) -> Value {
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
            "Name": LOGIN,
            "StableID": tailnet,
            "MagicDNSSuffix": "example.ts.net",
            "MagicDNSEnabled": true
        },
        "Peer": null
    })
}

/// A whois reply for the node `node` of the user `user`, with `tags`.
pub fn whois(node: &str, user: i64, tags: &[&str]) -> Value {
    json!({
        "Node": {
            "ID": 7,
            "StableID": node,
            "Name": PHONE_NAME,
            "Tags": tags,
            "Hostinfo": { "Hostname": "phone", "ShareeNode": false }
        },
        "UserProfile": { "ID": user, "LoginName": LOGIN, "DisplayName": "User" },
        "CapMap": {}
    })
}

/// A whois reply for a node that another tailnet shares with this one.
pub fn sharee_whois(node: &str, user: i64) -> Value {
    let mut whois = whois(node, user, &[]);
    whois["Node"]["Hostinfo"]["ShareeNode"] = Value::Bool(true);
    whois
}

#[derive(Debug, Default)]
struct Answers {
    status: Option<Value>,
    whois: Option<Value>,
    seen: Vec<String>,
}

/// A running fake tailscaled. Dropping it stops it and removes its socket.
#[derive(Debug)]
pub struct FakeTailscaled {
    socket: PathBuf,
    answers: Arc<Mutex<Answers>>,
    /// A line for every open watch; `None` ends them.
    lines: broadcast::Sender<Option<String>>,
    accept: JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl FakeTailscaled {
    /// Starts a fake whose status is [`status`] at [`SELF_IP`] and whose whois names
    /// the phone's node [`PHONE_NODE`] of [`USER`]. It needs a tokio runtime.
    ///
    /// # Errors
    ///
    /// [`TestSupportError::CreateTempDir`] or [`TestSupportError::BindSocket`] when
    /// its directory or its socket cannot be made.
    pub fn start() -> Result<FakeTailscaled, TestSupportError> {
        let dir =
            tempfile::tempdir().map_err(|source| TestSupportError::CreateTempDir { source })?;
        let socket = dir.path().join("tailscaled.sock");
        let listener = UnixListener::bind(&socket)
            .map_err(|source| TestSupportError::BindSocket { path: socket.clone(), source })?;
        let answers = Arc::new(Mutex::new(Answers {
            status: Some(status(SELF_IP, TAILNET)),
            whois: Some(whois(PHONE_NODE, USER, &[])),
            seen: Vec::new(),
        }));
        let (lines, _) = broadcast::channel(16);
        let serving = (Arc::clone(&answers), lines.clone());
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (answers, lines) = (Arc::clone(&serving.0), serving.1.clone());
                tokio::spawn(answer(stream, answers, lines));
            }
        });
        Ok(FakeTailscaled { socket, answers, lines, accept, _dir: dir })
    }

    /// The socket of the fake's LocalAPI, for `remote.tailscaled_socket`.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Sets the status; `None` makes `status` fail.
    pub fn set_status(&self, status: Option<Value>) {
        self.lock().status = status;
    }

    /// Sets the whois answer for every peer; `None` makes `whois` answer 404.
    pub fn set_whois(&self, whois: Option<Value>) {
        self.lock().whois = whois;
    }

    /// Sends `line` on every open watch.
    pub fn push(&self, line: &Value) {
        let _ = self.lines.send(Some(line.to_string()));
    }

    /// Ends every open watch.
    pub fn end_watches(&self) {
        let _ = self.lines.send(None);
    }

    /// The target of each request so far, such as `/localapi/v0/whois?addr=...`.
    pub fn seen(&self) -> Vec<String> {
        self.lock().seen.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Answers> {
        self.answers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for FakeTailscaled {
    fn drop(&mut self) {
        self.end_watches();
        self.accept.abort();
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

#[cfg(test)]
mod tests;
