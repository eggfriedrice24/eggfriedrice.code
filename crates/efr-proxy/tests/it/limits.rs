//! The caps of a call (DP10), the close with its grace, and the debug line of a call.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use efr_protocol::BlockReason;
use efr_proxy::{Guard, Limits};
use pretty_assertions::assert_eq;
use tracing::subscriber::DefaultGuard;

use crate::kit::{BUDGET, Client, Upstream, World, list, record, rustls_hello};

async fn world(limits: Limits) -> (World, Upstream) {
    let world =
        World::build(list(&["allowed.test"], &[], &[]), Guard::with_loopback(), Vec::new(), limits);
    let echo = Upstream::echo().await;
    world.table.add("allowed.test", &["127.0.0.1"]);
    world.connector.route("127.0.0.1:443", echo.addr);
    (world, echo)
}

#[tokio::test]
async fn proxy_caps_tunnels() {
    let (world, _echo) = world(Limits { connections: 2, ..Limits::default() }).await;
    let (call, path) = world.call(&[], BUDGET);
    let mut first = Client::connect(&path).await;
    assert_eq!(first.connect_to("allowed.test:443").await, 200);
    let mut second = Client::connect(&path).await;
    assert_eq!(second.connect_to("allowed.test:443").await, 200);
    let mut third = Client::connect(&path).await;
    assert_eq!(third.status().await, 503);
    let body = String::from_utf8(third.rest().await).unwrap();
    assert_eq!(body, "efr-proxy: too many connections\n");
    let (records, _) = call.blocked();
    assert_eq!(
        (records[0].host.as_str(), records[0].port, records[0].reason),
        ("<invalid>", 0, BlockReason::Limit)
    );
}

#[tokio::test]
async fn proxy_connection_flood_does_not_stall_other_call() {
    let (world, echo) = world(Limits::default()).await;
    let (flooded, flooded_path) = world.call(&[], BUDGET);
    let (_other, other_path) = world.call(&[], BUDGET);
    let mut silent = Vec::new();
    for _ in 0..Limits::default().connections {
        silent.push(Client::connect(&flooded_path).await);
    }
    // The flood holds every place of the first call ...
    let mut over = Client::connect(&flooded_path).await;
    assert_eq!(over.status().await, 503);
    assert_eq!(flooded.blocked().0[0].reason, BlockReason::Limit);
    // ... and the other call still serves at once.
    let serve = async {
        let mut client = Client::connect(&other_path).await;
        assert_eq!(client.connect_to("allowed.test:443").await, 200);
        let hello = rustls_hello("allowed.test", true);
        client.send(&hello).await;
        client.exact(hello.len()).await
    };
    let echoed = tokio::time::timeout(Duration::from_secs(5), serve).await.unwrap();
    assert_eq!(echoed.len(), rustls_hello("allowed.test", true).len());
    assert_eq!(echo.got().len(), 1);
}

#[tokio::test]
async fn proxy_caps_block_records() {
    let (world, _echo) = world(Limits::default()).await;
    let (call, path) = world.call(&[], BUDGET);
    for index in 0..33 {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(&format!("h{index}.test:443")).await, 403);
    }
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("h0.test:443").await, 403);
    let (records, more) = call.blocked();
    assert_eq!((records.len(), more), (32, 1));
    assert_eq!(records[0].count, 2);
    let report = call.close(Duration::ZERO).await;
    assert_eq!((report.blocked.len(), report.blocked_more), (32, 1));
}

#[tokio::test]
async fn proxy_close_waits_for_grace_then_cuts() {
    let (world, _echo) = world(Limits::default()).await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let hello = rustls_hello("allowed.test", true);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
    let grace = Duration::from_millis(200);
    let start = Instant::now();
    let report = tokio::time::timeout(Duration::from_secs(10), call.close(grace)).await.unwrap();
    assert!(start.elapsed() >= grace, "close waited {:?}", start.elapsed());
    assert!(client.closed().await, "the open tunnel is cut after the grace");
    assert_eq!(report.tunnels, 1);
    // The listener is gone.
    let refused = tokio::net::UnixStream::connect(&path).await.map(|_| ()).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
}

#[tokio::test]
async fn proxy_close_ends_at_once_when_the_tunnels_ended() {
    let (world, _echo) = world(Limits::default()).await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("other.test:443").await, 403);
    let _ = client.rest().await;
    let report =
        tokio::time::timeout(Duration::from_secs(5), call.close(Duration::from_secs(60))).await;
    assert_eq!(report.unwrap().blocked.len(), 1);
}

/// Every log line of this thread while its guard lives, as text.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn start() -> (Logs, DefaultGuard) {
        let logs = Logs::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || sink.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn proxy_logs_call_bytes() {
    let (logs, _guard) = Logs::start();
    let (world, _echo) = world(Limits::default()).await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let bytes = [rustls_hello("allowed.test", true), record(100)].concat();
    client.send(&bytes).await;
    assert_eq!(client.exact(bytes.len()).await, bytes);
    let mut refused = Client::connect(&path).await;
    assert_eq!(refused.connect_to("other.test:443").await, 403);
    drop(client);
    let report = call.close(Duration::from_secs(5)).await;
    assert_eq!(report.bytes_up, u64::try_from(bytes.len()).unwrap());
    let text = logs.text();
    let line = text
        .lines()
        .find(|line| line.contains("phase=\"proxy\""))
        .unwrap_or_else(|| panic!("no proxy line in:\n{text}"));
    for field in [
        format!("call={}", crate::kit::policy(&[], &[], 0).call),
        "tunnels=1".to_owned(),
        format!("bytes_up={}", bytes.len()),
        format!("bytes_down={}", bytes.len()),
        "blocked=1".to_owned(),
    ] {
        assert!(line.contains(&field), "{field} in {line}");
    }
}
