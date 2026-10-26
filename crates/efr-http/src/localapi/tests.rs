//! The LocalAPI client against a fake tailscaled on a Unix socket in a temporary
//! directory. Nothing reaches the real tailscaled.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixListener;

use super::{BackendState, IpnState, LocalApi, TailscaleVersion, WATCH_MASK, WATCH_MAX_LINE_BYTES};
use crate::HttpError;
use crate::testing::StoppedClock;

/// A fake tailscaled: answers each request with what `reply` gives for its target, then
/// closes the connection, and records each request head as lowercase text.
struct FakeTailscaled {
    socket: PathBuf,
    seen: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

impl FakeTailscaled {
    fn start(reply: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> FakeTailscaled {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tailscaled.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let reply = Arc::new(reply);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let record = record.clone();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buffer = [0; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        let read = stream.read(&mut buffer).await.unwrap();
                        if read == 0 {
                            return;
                        }
                        head.extend_from_slice(&buffer[..read]);
                    }
                    let head = String::from_utf8(head).unwrap();
                    let target = head.split(' ').nth(1).unwrap_or_default().to_owned();
                    record.lock().unwrap().push(head.to_lowercase());
                    stream.write_all(&reply(&target)).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        FakeTailscaled { socket, seen, _dir: dir }
    }

    fn client(&self) -> LocalApi {
        LocalApi::new(&self.socket, Arc::new(StoppedClock))
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// A reply with `status`, the version header when `version` is set, and `body`.
fn reply(status: &str, version: Option<&str>, body: &str) -> Vec<u8> {
    let version = version.map(|v| format!("tailscale-version: {v}\r\n")).unwrap_or_default();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{version}content-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A reply with no length, so the body runs until the connection closes, as the watch's
/// body does.
fn streamed(version: &str, lines: &[&str]) -> Vec<u8> {
    let mut text = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntailscale-version: {version}\r\nconnection: close\r\n\r\n"
    );
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text.into_bytes()
}

/// A status as tailscaled 1.104 sends it for `peers=false`, cut to the fields around
/// the ones efrd reads.
const STATUS: &str = r#"{
  "Version": "1.104.1-t0123abcd-g4567ef89",
  "TUN": true,
  "BackendState": "Running",
  "HaveNodeKey": true,
  "AuthURL": "",
  "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1234:5678"],
  "Self": {
    "ID": "nSELFxxCNTRL",
    "PublicKey": "nodekey:0000",
    "HostName": "desk",
    "DNSName": "desk.example.ts.net.",
    "OS": "linux",
    "UserID": 9007199254740993,
    "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1234:5678"],
    "Online": true
  },
  "Health": null,
  "MagicDNSSuffix": "example.ts.net",
  "CurrentTailnet": {
    "Name": "user@example.com",
    "StableID": "T1234CNTRL",
    "MagicDNSSuffix": "example.ts.net",
    "MagicDNSEnabled": true
  },
  "CertDomains": null,
  "Peer": null,
  "User": null,
  "ClientVersion": null
}"#;

#[tokio::test]
async fn status_asks_without_peers_and_reads_the_fields_efrd_checks() {
    let fake = FakeTailscaled::start(|_| reply("200 OK", Some("1.104.1-t0123abcd"), STATUS));

    let status = fake.client().status().await.unwrap();

    assert_eq!(status.backend_state, BackendState::Running);
    assert!(status.tun);
    assert!(status.is_running_with_tun());
    assert_eq!(status.tailnet_ipv4(), Some(Ipv4Addr::new(100, 101, 102, 103)));
    assert_eq!(status.self_stable_id(), Some("nSELFxxCNTRL"));
    assert_eq!(status.tailnet_stable_id(), Some("T1234CNTRL"));
    let node = status.self_node.as_ref().unwrap();
    assert_eq!(node.user_id, 9_007_199_254_740_993, "a user id above 2^53 stays exact");
    assert_eq!((node.host_name.as_str(), node.dns_name.as_str()), ("desk", "desk.example.ts.net."));
    assert!(node.tags.is_empty());
    let tailnet = status.current_tailnet.as_ref().unwrap();
    assert_eq!(tailnet.magic_dns_suffix, "example.ts.net");
    assert!(tailnet.magic_dns_enabled);
    let version = status.version.as_ref().unwrap();
    assert_eq!((version.triple(), version.long()), ((1, 104, 1), "1.104.1-t0123abcd"));

    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("get /localapi/v0/status?peers=false http/1.1\r\n"), "{seen:?}");
    assert!(seen[0].contains("\r\nhost: local-tailscaled.sock\r\n"), "{seen:?}");
    assert!(!seen[0].contains("origin:") && !seen[0].contains("referer:"), "{seen:?}");
}

#[tokio::test]
async fn own_addresses_match_also_when_mapped_into_ipv6() {
    let fake = FakeTailscaled::start(|_| reply("200 OK", None, STATUS));
    let status = fake.client().status().await.unwrap();

    assert!(status.is_own_ip("100.101.102.103".parse().unwrap()));
    assert!(status.is_own_ip("::ffff:100.101.102.103".parse().unwrap()));
    assert!(status.is_own_ip("fd7a:115c:a1e0::1234:5678".parse().unwrap()));
    assert!(!status.is_own_ip("100.101.102.104".parse().unwrap()));
    assert!(!status.is_own_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
}

#[tokio::test]
async fn without_the_header_the_version_comes_from_the_body() {
    let fake = FakeTailscaled::start(|_| reply("200 OK", None, STATUS));
    let status = fake.client().status().await.unwrap();
    assert_eq!(status.version.unwrap().triple(), (1, 104, 1));
}

#[tokio::test]
async fn userspace_networking_and_a_stopped_backend_are_not_running() {
    let body = r#"{"Version":"1.98.0","TUN":false,"BackendState":"Running",
        "TailscaleIPs":null,"Self":null,"CurrentTailnet":{"Name":"old"}}"#;
    let fake = FakeTailscaled::start(move |_| reply("200 OK", None, body));
    let status = fake.client().status().await.unwrap();
    assert!(!status.is_running_with_tun());
    assert_eq!(status.tailnet_ipv4(), None);
    assert_eq!(status.self_stable_id(), None);
    assert_eq!(status.tailnet_stable_id(), None, "an old tailscaled sends no StableID");
    assert!(!status.version.unwrap().at_least(TailscaleVersion::MINIMUM_REMOTE));

    let body = r#"{"TUN":true,"BackendState":"Stopped","TailscaleIPs":["100.64.0.1"]}"#;
    let fake = FakeTailscaled::start(move |_| reply("200 OK", None, body));
    let status = fake.client().status().await.unwrap();
    assert_eq!(status.backend_state, BackendState::Stopped);
    assert!(!status.is_running_with_tun());
    assert_eq!(status.version, None);

    let body = r#"{"TUN":true,"BackendState":"SomethingNew"}"#;
    let fake = FakeTailscaled::start(move |_| reply("200 OK", None, body));
    assert_eq!(fake.client().status().await.unwrap().backend_state, BackendState::Unknown);
}

#[test]
fn the_tailnet_ipv4_is_the_first_address_in_100_64_slash_10() {
    let status = |ips: &str| -> super::TailscaleStatus {
        serde_json::from_str(&format!(r#"{{"TailscaleIPs":{ips}}}"#)).unwrap()
    };
    assert_eq!(status(r#"["100.63.255.255","100.128.0.1"]"#).tailnet_ipv4(), None);
    assert_eq!(
        status(r#"["fd7a::1","100.64.0.0","100.127.255.255"]"#).tailnet_ipv4(),
        Some(Ipv4Addr::new(100, 64, 0, 0))
    );
    assert_eq!(
        status(r#"["192.168.1.2","100.127.255.255"]"#).tailnet_ipv4(),
        Some(Ipv4Addr::new(100, 127, 255, 255))
    );
}

#[tokio::test]
async fn a_failed_status_is_a_local_api_status_error() {
    let fake = FakeTailscaled::start(|_| reply("403 Forbidden", None, "access denied"));
    match fake.client().status().await {
        Err(HttpError::LocalApiStatus { url, status }) => {
            assert_eq!(status, 403);
            assert_eq!(url, "http://local-tailscaled.sock/localapi/v0/status?peers=false");
        }
        other => panic!("unexpected result {other:?}"),
    }
}

/// A whois reply for a phone that a user owns, cut to the fields around the ones efrd
/// reads. Its numeric node id is the largest int64, far above 2^53.
const WHOIS_PHONE: &str = r#"{
  "Node": {
    "ID": 9223372036854775807,
    "StableID": "nPHONExCNTRL",
    "Name": "iphone.example.ts.net.",
    "User": 9007199254740993,
    "Key": "nodekey:1111",
    "Addresses": ["100.70.1.2/32", "fd7a:115c:a1e0::aaaa/128"],
    "Hostinfo": {"OS": "iOS", "Hostname": "iPhone"},
    "Created": "2026-01-01T00:00:00Z",
    "ComputedName": "iphone"
  },
  "UserProfile": {
    "ID": 9007199254740993,
    "LoginName": "user@example.com",
    "DisplayName": "A User",
    "ProfilePicURL": ""
  },
  "CapMap": null
}"#;

#[tokio::test]
async fn whois_sends_the_peer_and_tcp_and_reads_the_identity() {
    let fake = FakeTailscaled::start(|_| reply("200 OK", None, WHOIS_PHONE));
    let peer: SocketAddr = "100.70.1.2:51234".parse().unwrap();

    let identity = fake.client().whois(peer).await.unwrap().unwrap();

    assert_eq!(identity.node_stable_id, "nPHONExCNTRL");
    assert_eq!(identity.node_name, "iphone.example.ts.net.");
    assert_eq!(identity.host_name, "iPhone");
    assert!(identity.tags.is_empty());
    assert!(!identity.sharee_node);
    assert!(!identity.is_tagged_or_shared());
    assert_eq!(identity.user_id, 9_007_199_254_740_993);
    assert_eq!(identity.login_name, "user@example.com");
    assert_eq!(identity.display_name, "A User");

    let seen = fake.seen();
    assert!(
        seen[0]
            .starts_with("get /localapi/v0/whois?addr=100.70.1.2%3a51234&proto=tcp http/1.1\r\n"),
        "{seen:?}"
    );
}

#[tokio::test]
async fn whois_unmaps_a_mapped_ipv4_peer_and_brackets_an_ipv6_peer() {
    let fake = FakeTailscaled::start(|_| reply("200 OK", None, WHOIS_PHONE));
    let client = fake.client();
    client.whois("[::ffff:100.70.1.2]:51234".parse().unwrap()).await.unwrap();
    client.whois("[fd7a:115c:a1e0::aaaa]:443".parse().unwrap()).await.unwrap();
    let seen = fake.seen();
    assert!(seen[0].contains("?addr=100.70.1.2%3a51234&proto=tcp "), "{seen:?}");
    assert!(
        seen[1].contains("?addr=%5bfd7a%3a115c%3aa1e0%3a%3aaaaa%5d%3a443&proto=tcp "),
        "{seen:?}"
    );
}

#[tokio::test]
async fn whois_of_an_unknown_peer_is_none() {
    let fake = FakeTailscaled::start(|_| reply("404 Not Found", None, "no match for IP:port"));
    let peer: SocketAddr = "100.70.9.9:1".parse().unwrap();
    assert_eq!(fake.client().whois(peer).await.unwrap(), None);
}

#[tokio::test]
async fn whois_reads_tags_and_sharee_nodes() {
    let tagged = r#"{"Node":{"StableID":"nTAG","Name":"ci.example.ts.net.","Tags":["tag:ci"],
        "Hostinfo":{}},"UserProfile":{"ID":1,"LoginName":"tagged-devices"}}"#;
    let shared = r#"{"Node":{"StableID":"nSHARE","Name":"x.other.ts.net.","Tags":null,
        "Hostinfo":{"ShareeNode":true}},"UserProfile":{"ID":2,"LoginName":"other@x"}}"#;
    let fake = FakeTailscaled::start(move |target| {
        reply("200 OK", None, if target.contains("100.70.0.1") { tagged } else { shared })
    });
    let client = fake.client();
    let tagged = client.whois("100.70.0.1:1".parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(tagged.tags, ["tag:ci"]);
    assert!(tagged.is_tagged_or_shared());
    let shared = client.whois("100.70.0.2:1".parse().unwrap()).await.unwrap().unwrap();
    assert!(shared.sharee_node && shared.tags.is_empty());
    assert!(shared.is_tagged_or_shared());
}

#[tokio::test]
async fn a_partial_whois_reply_fails_closed() {
    let cases = [
        r#"{"Node":null,"UserProfile":{"ID":1}}"#,
        r#"{"Node":{"StableID":"nX"}}"#,
        r#"{"Node":{"StableID":"nX"},"UserProfile":{"LoginName":"no id"}}"#,
    ];
    for body in cases {
        let fake = FakeTailscaled::start(move |_| reply("200 OK", None, body));
        let result = fake.client().whois("100.70.0.1:1".parse().unwrap()).await;
        assert!(matches!(result, Err(HttpError::DecodeJson { .. })), "{body}: {result:?}");
    }
    let body = r#"{"Node":{"StableID":" "},"UserProfile":{"ID":1}}"#;
    let fake = FakeTailscaled::start(move |_| reply("200 OK", None, body));
    match fake.client().whois("100.70.0.1:1".parse().unwrap()).await {
        Err(HttpError::LocalApiReply { problem, .. }) => {
            assert_eq!(problem, "the node has no stable id");
        }
        other => panic!("unexpected result {other:?}"),
    }
}

#[tokio::test]
async fn whois_with_another_failure_status_is_an_error() {
    let fake =
        FakeTailscaled::start(|_| reply("400 Bad Request", None, "invalid 'addr' parameter"));
    let result = fake.client().whois("100.70.0.1:1".parse().unwrap()).await;
    assert!(matches!(result, Err(HttpError::LocalApiStatus { status, .. }) if status == 400));
}

#[tokio::test]
async fn the_watch_reads_each_message_until_a_lagging_error_and_the_end() {
    let initial = format!(
        r#"{{"Version":"1.104.1-tabc","ErrMessage":null,"LoginFinished":null,"State":6,"Prefs":null,"InitialStatus":{}}}"#,
        STATUS.replace('\n', "")
    );
    let lines = [
        initial,
        r#"{"Version":"","ErrMessage":null,"State":null,"Engine":null,"BrowseToURL":null}"#
            .to_owned(),
        r#"{"Version":"","SelfChange":{"ID":1,"StableID":"nSELFxxCNTRL","Name":"desk.example.ts.net.","Addresses":["fd7a:115c:a1e0::1234:5678/128","100.101.102.200/32"]}}"#.to_owned(),
        r#"{"Version":"","State":4}"#.to_owned(),
        r#"{"Version":"","ErrMessage":"IPN bus consumer fell behind"}"#.to_owned(),
    ];
    let fake = FakeTailscaled::start(move |_| {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        streamed("1.104.1-tabc", &refs)
    });

    let mut watch = fake.client().watch().await.unwrap();
    assert_eq!(watch.version().unwrap().triple(), (1, 104, 1));

    let first = watch.next().await.unwrap().unwrap();
    assert_eq!(first.version.as_deref(), Some("1.104.1-tabc"));
    assert_eq!(first.state, Some(IpnState::Running));
    let status = first.initial_status.unwrap();
    assert_eq!(status.tailnet_ipv4(), Some(Ipv4Addr::new(100, 101, 102, 103)));
    assert_eq!(status.tailnet_stable_id(), Some("T1234CNTRL"));
    assert_eq!(status.version.unwrap().triple(), (1, 104, 1));

    let empty = watch.next().await.unwrap().unwrap();
    assert_eq!((empty.version, empty.err_message, empty.state), (None, None, None));
    assert!(empty.self_change.is_none() && empty.initial_status.is_none());

    let change = watch.next().await.unwrap().unwrap().self_change.unwrap();
    assert_eq!(change.stable_id, "nSELFxxCNTRL");
    assert_eq!(change.tailnet_ipv4(), Some(Ipv4Addr::new(100, 101, 102, 200)));

    assert_eq!(watch.next().await.unwrap().unwrap().state, Some(IpnState::Stopped));

    let last = watch.next().await.unwrap().unwrap();
    assert!(last.fell_behind());
    assert_eq!(last.err_message.as_deref(), Some("IPN bus consumer fell behind"));
    assert_eq!(watch.next().await.unwrap(), None);

    let seen = fake.seen();
    assert!(
        seen[0].starts_with(&format!("get /localapi/v0/watch-ipn-bus?mask={WATCH_MASK} ")),
        "{seen:?}"
    );
    assert_eq!(WATCH_MASK, 16_386);
    assert_eq!(WATCH_MASK & 8, 0, "the obsolete net map bit is refused by tailscaled");
    assert_eq!(WATCH_MAX_LINE_BYTES, 8 * 1024 * 1024);
}

#[tokio::test]
async fn a_refused_watch_is_a_local_api_status_error() {
    let fake = FakeTailscaled::start(|_| reply("400 Bad Request", None, "bad mask"));
    let result = fake.client().watch().await;
    assert!(matches!(result, Err(HttpError::LocalApiStatus { status, .. }) if status == 400));
}

#[test]
fn ipn_states_come_from_their_numbers() {
    let states: Vec<IpnState> = (0..=7).map(IpnState::from_number).collect();
    assert_eq!(
        states,
        [
            IpnState::NoState,
            IpnState::InUseOtherUser,
            IpnState::NeedsLogin,
            IpnState::NeedsMachineAuth,
            IpnState::Stopped,
            IpnState::Starting,
            IpnState::Running,
            IpnState::Unknown(7),
        ]
    );
}

#[tokio::test]
async fn a_missing_socket_is_a_connect_error() {
    let dir = tempfile::tempdir().unwrap();
    let client = LocalApi::new(dir.path().join("absent.sock"), Arc::new(StoppedClock));
    assert_eq!(client.socket(), dir.path().join("absent.sock"));
    assert!(matches!(client.status().await, Err(HttpError::UnixConnect { .. })));
    assert!(matches!(
        client.whois("100.70.0.1:1".parse().unwrap()).await,
        Err(HttpError::UnixConnect { .. })
    ));
    assert!(matches!(client.watch().await, Err(HttpError::UnixConnect { .. })));
}
