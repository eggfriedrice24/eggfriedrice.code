use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

use super::{FakeTailscaled, PHONE_NODE, SELF_IP, TAILNET, USER, sharee_whois, status, whois};

/// Sends a GET of `target` and returns the status line and the body.
async fn get(fake: &FakeTailscaled, target: &str) -> (String, String) {
    let mut stream = UnixStream::connect(fake.socket()).await.unwrap();
    let head = format!("GET {target} HTTP/1.1\r\nHost: local-tailscaled.sock\r\n\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).await.unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_owned(), body.to_owned())
}

#[tokio::test]
async fn the_fake_answers_the_status_and_the_whois_that_the_test_set() {
    let fake = FakeTailscaled::start().unwrap();
    let (line, body) = get(&fake, "/localapi/v0/status?peers=false").await;
    assert_eq!(line, "HTTP/1.1 200 OK");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), status(SELF_IP, TAILNET));

    let whois_target = "/localapi/v0/whois?addr=127.0.0.1:5000&proto=tcp";
    let (line, body) = get(&fake, whois_target).await;
    assert_eq!(line, "HTTP/1.1 200 OK");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), whois(PHONE_NODE, USER, &[]));

    fake.set_whois(None);
    let (line, _) = get(&fake, whois_target).await;
    assert_eq!(line, "HTTP/1.1 404 Not Found");
    fake.set_status(None);
    let (line, _) = get(&fake, "/localapi/v0/status").await;
    assert_eq!(line, "HTTP/1.1 503 Service Unavailable");
    assert_eq!(
        fake.seen(),
        ["/localapi/v0/status?peers=false", whois_target, whois_target, "/localapi/v0/status"]
    );
}

#[tokio::test]
async fn the_watch_sends_the_initial_status_then_each_pushed_line_until_it_ends() {
    let fake = FakeTailscaled::start().unwrap();
    let mut stream = UnixStream::connect(fake.socket()).await.unwrap();
    let head = "GET /localapi/v0/watch-ipn-bus?mask=16386 HTTP/1.1\r\nHost: x\r\n\r\n";
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut lines = BufReader::new(stream).lines();
    while !lines.next_line().await.unwrap().unwrap().is_empty() {}
    let first: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(first["InitialStatus"], status(SELF_IP, TAILNET));
    // NOTE: the watch subscribes before it writes its first line, so no push is lost.
    fake.push(&json!({ "State": 6 }));
    let next: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(next, json!({ "State": 6 }));
    fake.end_watches();
    assert_eq!(lines.next_line().await.unwrap(), None);
}

#[test]
fn a_sharee_whois_differs_from_a_whois_only_in_its_mark() {
    let mut shared = sharee_whois(PHONE_NODE, USER);
    assert_eq!(shared["Node"]["Hostinfo"]["ShareeNode"], json!(true));
    shared["Node"]["Hostinfo"]["ShareeNode"] = json!(false);
    assert_eq!(shared, whois(PHONE_NODE, USER, &[]));
}
