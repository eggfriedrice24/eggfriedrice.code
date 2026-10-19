//! `CONNECT`: the lists before any lookup, the host forms, the address guard, the
//! connect to the checked address, the ClientHello and its server name.

use std::sync::Arc;
use std::time::Duration;

use efr_permissions::HostTable;
use efr_protocol::{BlockReason, Blocked};
use efr_proxy::{Guard, Limits};
use pretty_assertions::assert_eq;

use crate::kit::{
    BUDGET, Client, GLOBAL_V4, GLOBAL_V4_OWN, GLOBAL_V6, GLOBAL_V6_OWN, Silent, Upstream, World,
    ip, list, policy, record, rustls_hello, short, with_extension,
};

fn blocked(host: &str, port: u16, reason: BlockReason) -> Blocked {
    Blocked { host: host.to_owned(), port, reason, count: 1 }
}

/// A world that lists `allowed.test`, resolves it to 127.0.0.1 and routes port 443
/// there to an echo server.
async fn echo_world() -> (World, Upstream) {
    let world = World::new(list(&["allowed.test"], &[], &[]));
    let echo = Upstream::echo().await;
    world.table.add("allowed.test", &["127.0.0.1"]);
    world.connector.route("127.0.0.1:443", echo.addr);
    (world, echo)
}

#[tokio::test]
async fn proxy_allows_listed_host() {
    let (world, echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let hello = rustls_hello("allowed.test", true);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello, "the hello reached the echo whole");
    let data = record(300);
    client.send(&data).await;
    assert_eq!(client.exact(data.len()).await, data);
    assert_eq!(world.connector.seen(), vec!["127.0.0.1:443".parse().unwrap()]);
    assert_eq!(world.table.asked(), vec!["allowed.test".to_owned()]);
    drop(client);
    let report = call.close(Duration::from_secs(5)).await;
    assert_eq!(report.blocked, Vec::new());
    assert_eq!(report.tunnels, 1);
    assert_eq!(report.bytes_up, u64::try_from(hello.len() + data.len()).unwrap());
    assert_eq!(report.bytes_down, report.bytes_up);
    assert_eq!(echo.got(), vec![[hello, data].concat()]);
}

#[tokio::test]
async fn proxy_denies_unlisted_403() {
    let (world, echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("other.test:443").await, 403);
    let body = String::from_utf8(client.rest().await).unwrap();
    assert!(body.contains("not_allowed"), "{body}");
    assert_eq!(call.blocked(), (vec![blocked("other.test", 443, BlockReason::NotAllowed)], 0));
    assert_eq!(world.table.asked(), Vec::<String>::new(), "no lookup for an unlisted host");
    assert_eq!(world.connector.seen(), Vec::new());
    assert_eq!(echo.total(), 0);
}

#[tokio::test]
async fn proxy_denied_host_never_resolves() {
    let world = World::new(list(&["*.example.test"], &["bad.example.test"], &[]));
    let (call, path) = world.call(&[], BUDGET);
    for target in ["bad.example.test:443", "unlisted.test:443", "example.test:443"] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(target).await, 403, "{target}");
    }
    assert_eq!(world.table.asked(), Vec::<String>::new());
    // A name below the listed suffix is looked up: here it has no address.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("good.example.test:443").await, 502);
    assert_eq!(world.table.asked(), vec!["good.example.test".to_owned()]);
    let (records, _) = call.blocked();
    assert_eq!(
        records.iter().map(|record| record.host.as_str()).collect::<Vec<_>>(),
        vec!["bad.example.test", "unlisted.test", "example.test"]
    );
}

#[tokio::test]
async fn proxy_deny_wins_over_grant() {
    let (world, _echo) = echo_world().await;
    world.hosts.send_replace(Arc::new(list(&["allowed.test"], &["granted.test"], &[])));
    world.table.add("granted.test", &["127.0.0.1"]);
    // The global deny list wins over the call's grant.
    let (call, path) = world.call(&[("granted.test", 443)], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("granted.test:443").await, 403);
    assert_eq!(call.blocked().0, vec![blocked("granted.test", 443, BlockReason::NotAllowed)]);
    // The call's own deny set wins over the global allow list.
    let (call, path) = world.call_with(policy(&[], &["Allowed.Test."], BUDGET));
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 403);
    assert_eq!(call.blocked().0, vec![blocked("allowed.test", 443, BlockReason::NotAllowed)]);
    assert_eq!(world.table.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn proxy_grant_is_per_call() {
    let (world, _echo) = echo_world().await;
    world.table.add("extra.test", &["127.0.0.1"]);
    let (first, first_path) = world.call(&[("extra.test", 443)], BUDGET);
    let (second, second_path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&first_path).await;
    assert_eq!(client.connect_to("extra.test:443").await, 200);
    let mut client = Client::connect(&second_path).await;
    assert_eq!(client.connect_to("extra.test:443").await, 403);
    assert_eq!(first.blocked(), (Vec::new(), 0));
    assert_eq!(second.blocked().0, vec![blocked("extra.test", 443, BlockReason::NotAllowed)]);
    // A grant is for its one port.
    let mut client = Client::connect(&first_path).await;
    assert_eq!(client.connect_to("extra.test:8443").await, 403);
}

#[tokio::test]
async fn proxy_refuses_private_resolution() {
    let world = World::build(
        list(&["*.names.test"], &[], &[]),
        Guard::strict(),
        vec![(ip(GLOBAL_V4_OWN), 32)],
        Limits::default(),
    );
    let (call, path) = world.call(&[], BUDGET);
    let classes = [
        "127.0.0.1",
        "::1",
        "0.0.0.0",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "100.64.0.1",
        "169.254.169.254",
        "fe80::1",
        "fd00::1",
        "fec0::1",
        "224.0.0.1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::10.0.0.1",
        "64:ff9b::a00:1",
        "2002:a00:1::1",
        "192.0.2.1",
        "2001:db8::1",
        GLOBAL_V4_OWN,
    ];
    let mut names = Vec::new();
    for (index, address) in classes.iter().enumerate() {
        let name = format!("n{index}.names.test");
        world.table.add(&name, &[address]);
        names.push(name);
    }
    world.table.add("mixed.names.test", &[GLOBAL_V4, "10.0.0.1"]);
    names.push("mixed.names.test".to_owned());
    for name in &names {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(&format!("{name}:443")).await, 403, "{name}");
    }
    assert_eq!(world.connector.seen(), Vec::new(), "no connect to a refused address");
    let (records, more) = call.blocked();
    assert_eq!(more, 0);
    assert_eq!(records.len(), names.len());
    assert!(records.iter().all(|record| record.reason == BlockReason::PrivateAddress));
    // A global address passes the guard: the test connector then refuses it, so no
    // byte leaves.
    world.table.add("global.names.test", &[GLOBAL_V4, GLOBAL_V6]);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("global.names.test:443").await, 502);
    assert_eq!(
        world.connector.seen(),
        vec![
            format!("{GLOBAL_V4}:443").parse().unwrap(),
            format!("[{GLOBAL_V6}]:443").parse().unwrap()
        ]
    );
}

#[tokio::test]
async fn proxy_private_entry_reaches_lan() {
    let hosts = [HostTable { host: "*.lan.test", port: None, http: false, private: true }];
    let world = World::build(
        list(&[], &[], &hosts),
        Guard::strict(),
        vec![(ip(GLOBAL_V4_OWN), 24)],
        Limits::default(),
    );
    world.table.add("mirror.lan.test", &["10.0.0.1"]);
    world.table.add("ula.lan.test", &["fd00::5"]);
    world.table.add("neighbour.lan.test", &["11.0.1.7"]);
    for (name, address) in
        [("loopback", "127.0.0.1"), ("link", "169.254.1.1"), ("own", GLOBAL_V4_OWN)]
    {
        world.table.add(&format!("{name}.lan.test"), &[address]);
    }
    let (call, path) = world.call(&[], BUDGET);
    for name in ["mirror", "ula", "neighbour"] {
        let mut client = Client::connect(&path).await;
        // The guard passes; the test connector refuses every address that is not
        // loopback, so the answer is a 502.
        assert_eq!(client.connect_to(&format!("{name}.lan.test:443")).await, 502, "{name}");
    }
    for name in ["loopback", "link", "own"] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(&format!("{name}.lan.test:443")).await, 403, "{name}");
    }
    assert_eq!(
        world.connector.seen(),
        vec![
            "10.0.0.1:443".parse().unwrap(),
            "[fd00::5]:443".parse().unwrap(),
            "11.0.1.7:443".parse().unwrap()
        ]
    );
    assert_eq!(call.blocked().0.len(), 3);
}

#[tokio::test]
async fn proxy_refuses_on_link_neighbour() {
    let own = vec![(ip(GLOBAL_V6_OWN), 64), (ip(GLOBAL_V4_OWN), 24)];
    let hosts = [HostTable { host: "lan.test", port: None, http: false, private: true }];
    let world =
        World::build(list(&["*.names.test"], &[], &hosts), Guard::strict(), own, Limits::default());
    world.table.add("six.names.test", &["2a00:0:0:2::99"]);
    world.table.add("four.names.test", &["11.0.1.200"]);
    world.table.add("far.names.test", &["2a00:0:0:3::1"]);
    world.table.add("lan.test", &["2a00:0:0:2::99"]);
    let (call, path) = world.call(&[], BUDGET);
    for name in ["six.names.test", "four.names.test"] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(&format!("{name}:443")).await, 403, "{name}");
    }
    assert_eq!(world.connector.seen(), Vec::new());
    for name in ["far.names.test", "lan.test"] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(&format!("{name}:443")).await, 502, "{name}");
    }
    assert_eq!(world.connector.seen().len(), 2);
    assert!(call.blocked().0.iter().all(|record| record.reason == BlockReason::PrivateAddress));
}

#[tokio::test]
async fn proxy_connects_to_the_checked_address() {
    let (world, echo) = echo_world().await;
    world.table.answers("allowed.test", vec![vec![ip("127.0.0.1")], vec![ip("10.0.0.1")]]);
    let (_call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let hello = rustls_hello("allowed.test", true);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
    assert_eq!(world.table.asked(), vec!["allowed.test".to_owned()], "one lookup");
    assert_eq!(world.connector.seen(), vec!["127.0.0.1:443".parse().unwrap()]);
    assert_eq!(echo.got().len(), 1);
}

#[tokio::test]
async fn proxy_refuses_ip_literal() {
    let hosts = [HostTable { host: "127.0.0.1", port: None, http: false, private: false }];
    let world = World::new(list(&["allowed.test"], &[], &hosts));
    let echo = Upstream::echo().await;
    world.connector.route("127.0.0.1:443", echo.addr);
    let (call, path) = world.call(&[], BUDGET);
    for (target, host) in [
        ("192.0.2.1:443", "192.0.2.1"),
        ("[2001:db8::1]:443", "2001:db8::1"),
        ("2130706433:443", "2130706433"),
        ("0x7f000001:443", "0x7f000001"),
        ("127.1:443", "127.1"),
        ("0177.0.0.1:443", "0177.0.0.1"),
        ("0x7f.0.0.1:443", "0x7f.0.0.1"),
        ("127.0.0.1:8443", "127.0.0.1"),
    ] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(target).await, 403, "{target}");
        let (records, _) = call.blocked();
        let last = records.last().unwrap();
        assert_eq!((last.host.as_str(), last.reason), (host, BlockReason::IpLiteral));
    }
    assert_eq!(world.table.asked(), Vec::<String>::new());
    assert_eq!(world.connector.seen(), Vec::new());
    // A listed address passes with no lookup; its hello names no server.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("127.0.0.1:443").await, 200);
    let hello = rustls_hello("127.0.0.1", true);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
    assert_eq!(world.table.asked(), Vec::<String>::new());
    // A server name to an address is a mismatch.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("127.0.0.1:443").await, 200);
    client.send(&rustls_hello("allowed.test", true)).await;
    assert!(client.closed().await);
    assert_eq!(call.blocked().0.last().map(|record| record.reason), Some(BlockReason::SniMismatch));
}

#[tokio::test]
async fn proxy_cuts_sni_mismatch() {
    let (world, echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    client.send(&rustls_hello("b.test", true)).await;
    assert!(client.closed().await, "the tunnel is cut");
    assert_eq!(call.blocked().0, vec![blocked("allowed.test", 443, BlockReason::SniMismatch)]);
    echo.settled(1).await;
    assert_eq!(echo.total(), 0, "no client byte reached the upstream");
    // Case and one trailing dot do not count.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let hello = rustls_hello("Allowed.Test.", true);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
}

#[tokio::test]
async fn proxy_cuts_non_tls() {
    let (world, echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let hello = rustls_hello("allowed.test", true);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("http", b"GET / HTTP/1.1\r\nHost: allowed.test\r\n\r\n".to_vec()),
        ("ssh", b"SSH-2.0-OpenSSH_9.9\r\n".to_vec()),
        ("no sni", rustls_hello("allowed.test", false)),
        ("ech", with_extension(&hello, 0xfe0d, 32)),
        ("ech draft", with_extension(&hello, 0xfe08, 32)),
        ("esni", with_extension(&hello, 0xffce, 32)),
        ("not a hello", b"\x16\x03\x01\x00\x05hello".to_vec()),
        ("record too long", b"\x16\x03\x01\x40\x01".to_vec()),
    ];
    for (name, bytes) in &cases {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to("allowed.test:443").await, 200, "{name}");
        client.send(bytes).await;
        assert!(client.closed().await, "{name} is cut");
    }
    echo.settled(cases.len()).await;
    assert_eq!(echo.total(), 0, "no client byte reached the upstream");
    let (records, more) = call.blocked();
    assert_eq!((records.len(), more), (1, 0));
    assert_eq!(records[0].reason, BlockReason::NotTls);
    assert_eq!(records[0].count, u32::try_from(cases.len()).unwrap());
    // A hello over two writes completes.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    let (first, second) = hello.split_at(hello.len() / 2);
    client.send(first).await;
    client.send(second).await;
    assert_eq!(client.exact(hello.len()).await, hello);
}

#[tokio::test]
async fn proxy_cuts_a_hello_over_its_cap_and_a_silent_client() {
    let limits = short();
    assert_eq!(limits.hello_bytes, 16 * 1024);
    let world =
        World::build(list(&["allowed.test"], &[], &[]), Guard::with_loopback(), Vec::new(), limits);
    let echo = Upstream::echo().await;
    world.table.add("allowed.test", &["127.0.0.1"]);
    world.connector.route("127.0.0.1:443", echo.addr);
    let (call, path) = world.call(&[], BUDGET);
    // A hello in records of 1000 bytes that ends after the 16 KiB cap.
    let big = with_extension(&rustls_hello("allowed.test", true), 0x0015, 17 * 1024);
    let mut split = Vec::new();
    for fragment in big[5..].chunks(1000) {
        split.extend_from_slice(&[22, 3, 1]);
        split.extend_from_slice(&u16::try_from(fragment.len()).unwrap().to_be_bytes());
        split.extend_from_slice(fragment);
    }
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    client.send(&split).await;
    assert!(client.closed().await);
    // A client that sends nothing after the 200.
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    assert!(client.closed().await, "the hello time limit cuts the tunnel");
    echo.settled(2).await;
    assert_eq!(echo.total(), 0);
    let (records, _) = call.blocked();
    assert_eq!(
        records,
        vec![Blocked { count: 2, ..blocked("allowed.test", 443, BlockReason::NotTls) }]
    );
}

#[tokio::test]
async fn proxy_port_rules() {
    let hosts = [
        HostTable { host: "http.test", port: None, http: true, private: false },
        HostTable { host: "port.test", port: Some(8443), http: false, private: false },
        HostTable { host: "lan.test", port: Some(8080), http: true, private: false },
        HostTable { host: "both.test", port: Some(8080), http: true, private: false },
        HostTable { host: "both.test", port: Some(8080), http: false, private: false },
    ];
    let world = World::new(list(&["plain.test"], &[], &hosts));
    let echo = Upstream::echo().await;
    for name in ["plain.test", "http.test", "port.test", "grant.test", "lan.test", "both.test"] {
        world.table.add(name, &["127.0.0.1"]);
    }
    world.connector.route("127.0.0.1:443", echo.addr);
    world.connector.route("127.0.0.1:8443", echo.addr);
    world.connector.route("127.0.0.1:8080", echo.addr);
    let (call, path) = world.call(&[("grant.test", 80)], BUDGET);
    for (target, status, reason) in [
        ("plain.test:443", 200, None),
        ("plain.test:8443", 403, Some(BlockReason::NotAllowed)),
        ("plain.test:80", 403, Some(BlockReason::NotAllowed)),
        ("http.test:443", 200, None),
        ("http.test:80", 403, Some(BlockReason::Method)),
        ("port.test:8443", 200, None),
        ("port.test:443", 403, Some(BlockReason::NotAllowed)),
        ("grant.test:80", 403, Some(BlockReason::Method)),
        ("grant.test:443", 403, Some(BlockReason::NotAllowed)),
        // NOTE: an `http` entry with a port opens plain GET and HEAD there, no tunnel.
        ("lan.test:8080", 403, Some(BlockReason::Method)),
        ("both.test:8080", 200, None),
    ] {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(target).await, status, "{target}");
        if let Some(reason) = reason {
            assert_eq!(
                call.blocked().0.last().map(|record| record.reason),
                Some(reason),
                "{target}"
            );
        }
    }
}

#[tokio::test]
async fn proxy_live_list_change() {
    let (world, _echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut first = Client::connect(&path).await;
    assert_eq!(first.connect_to("allowed.test:443").await, 200);
    world.hosts.send_replace(Arc::new(list(&["other.test"], &[], &[])));
    let mut second = Client::connect(&path).await;
    assert_eq!(second.connect_to("allowed.test:443").await, 403);
    assert_eq!(call.blocked().0, vec![blocked("allowed.test", 443, BlockReason::NotAllowed)]);
}

#[tokio::test]
async fn proxy_refuses_control_chars() {
    let (world, _echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    for head in [
        &b"CONNECT allowed\x01.test:443 HTTP/1.1\r\n\r\n"[..],
        b"CONNECT allowed.test:443 HTTP/1.1\r\nUser-Agent: a\x1b[31m\r\n\r\n",
        b"GET http://allowed.test/\x7f HTTP/1.1\r\n\r\n",
    ] {
        let mut client = Client::connect(&path).await;
        client.send(head).await;
        assert_eq!(client.status().await, 400);
    }
    assert_eq!(
        call.blocked().0,
        vec![Blocked { count: 3, ..blocked("<invalid>", 0, BlockReason::BadRequest) }]
    );
    assert_eq!(world.table.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn proxy_record_host_is_checked_text() {
    let (world, _echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    client.send(b"CONNECT a\x00b.test:443 HTTP/1.1\r\n\r\n").await;
    assert_eq!(client.status().await, 400);
    let mut client = Client::connect(&path).await;
    client.send("CONNECT b\u{e9}.test:443 HTTP/1.1\r\n\r\n".as_bytes()).await;
    assert_eq!(client.status().await, 400);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("Other.Test.:443").await, 403);
    assert_eq!(
        call.blocked().0,
        vec![
            Blocked { count: 2, ..blocked("<invalid>", 0, BlockReason::BadRequest) },
            blocked("other.test", 443, BlockReason::NotAllowed),
        ]
    );
}

#[tokio::test]
async fn proxy_request_head_limit() {
    let (world, _echo) = echo_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    let head = format!(
        "CONNECT allowed.test:443 HTTP/1.1\r\nUser-Agent: {}\r\n\r\n",
        "x".repeat(Limits::default().head_bytes)
    );
    client.send(head.as_bytes()).await;
    assert_eq!(client.status().await, 400);
    assert_eq!(call.blocked().0, vec![blocked("<invalid>", 0, BlockReason::BadRequest)]);
    assert_eq!(world.table.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn proxy_head_timeout() {
    let world = World::build(
        list(&["allowed.test"], &[], &[]),
        Guard::with_loopback(),
        Vec::new(),
        short(),
    );
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    client.send(b"CONNECT allowed.test:443 HTTP/1.1\r\n").await;
    assert_eq!(client.status().await, 408);
    let body = String::from_utf8(client.rest().await).unwrap();
    assert_eq!(body, "efr-proxy: the request head took too long\n");
    let mut silent = Client::connect(&path).await;
    assert_eq!(silent.status().await, 408);
    assert_eq!(call.blocked(), (Vec::new(), 0), "a slow head names no host");
}

#[tokio::test]
async fn proxy_resolve_timeout_is_502_with_reason() {
    let world = World::with_resolver(
        list(&["allowed.test"], &[], &[]),
        Guard::with_loopback(),
        Vec::new(),
        short(),
        Some(Arc::new(Silent)),
    );
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 502);
    let body = String::from_utf8(client.rest().await).unwrap();
    assert_eq!(body, "efr-proxy: timed out\n");
    assert_eq!(call.blocked(), (Vec::new(), 0));
    assert_eq!(world.connector.seen(), Vec::new());
}

#[tokio::test]
async fn proxy_upstream_failure_is_502_without_record() {
    let world = World::new(list(&["allowed.test", "missing.test"], &[], &[]));
    world.table.add("allowed.test", &["127.0.0.1"]);
    let (call, path) = world.call(&[], BUDGET);
    for (target, reason) in [("allowed.test:443", "refused"), ("missing.test:443", "no such host")]
    {
        let mut client = Client::connect(&path).await;
        assert_eq!(client.connect_to(target).await, 502, "{target}");
        let body = String::from_utf8(client.rest().await).unwrap();
        assert_eq!(body, format!("efr-proxy: {reason}\n"));
    }
    assert_eq!(call.blocked(), (Vec::new(), 0));
}
