//! Plain HTTP: `GET` and `HEAD` only, only to an http host, one request per
//! connection, and a head that the proxy writes itself.

use efr_permissions::{HostTable, MirrorHost};
use efr_protocol::BlockReason;
use pretty_assertions::assert_eq;

use crate::kit::{BUDGET, Client, Upstream, World, list_with, policy};

/// A world with an http entry `http.test`, a TLS-only `tls.test`, an http pacman
/// mirror `mirror.test`, and `grant.test` for a grant on port 80. Each resolves to
/// 127.0.0.1, whose port 80 is the http server.
async fn http_world() -> (World, Upstream) {
    let hosts = [HostTable { host: "http.test", port: None, http: true, private: false }];
    let mirrors = [MirrorHost { host: "mirror.test".to_owned(), port: 80, http: true }];
    let world = World::new(list_with(&["tls.test", "@pacman-mirrors"], &[], &hosts, &mirrors));
    let server = Upstream::http().await;
    for name in ["http.test", "tls.test", "mirror.test", "grant.test"] {
        world.table.add(name, &["127.0.0.1"]);
    }
    world.connector.route("127.0.0.1:80", server.addr);
    (world, server)
}

async fn ask(path: &std::path::Path, head: &str) -> (u16, String) {
    let mut client = Client::connect(path).await;
    client.send(head.as_bytes()).await;
    let status = client.status().await;
    (status, String::from_utf8_lossy(&client.rest().await).into_owned())
}

#[tokio::test]
async fn proxy_plain_http_get_only() {
    let (world, server) = http_world().await;
    let (call, path) = world.call(&[("grant.test", 80)], BUDGET);
    for (head, status) in [
        ("GET http://http.test/ HTTP/1.1\r\nHost: http.test\r\n\r\n", 200),
        ("HEAD http://http.test/x HTTP/1.1\r\n\r\n", 200),
        ("GET http://mirror.test/core/os/x86_64/core.db HTTP/1.1\r\n\r\n", 200),
        ("GET http://grant.test/ HTTP/1.1\r\n\r\n", 200),
        ("POST http://http.test/ HTTP/1.1\r\nContent-Length: 0\r\n\r\n", 405),
        ("PUT http://http.test/ HTTP/1.1\r\nContent-Length: 0\r\n\r\n", 405),
        ("GET http://http.test/ HTTP/1.1\r\nContent-Length: 3\r\n\r\n", 400),
        ("GET http://http.test/ HTTP/1.1\r\nHost: other.test\r\n\r\n", 400),
        ("GET http://tls.test/ HTTP/1.1\r\n\r\n", 403),
        ("GET http://other.test/ HTTP/1.1\r\n\r\n", 403),
        ("CONNECT grant.test:80 HTTP/1.1\r\n\r\n", 403),
    ] {
        let (got, body) = ask(&path, head).await;
        assert_eq!(got, status, "{head:?}");
        if status == 200 {
            assert_eq!(body, "ok", "{head:?}");
        }
    }
    let heads = server.got();
    assert_eq!(heads.len(), 4, "only the allowed requests reached the server");
    let reasons: Vec<_> =
        call.blocked().0.iter().map(|record| (record.host.clone(), record.reason)).collect();
    assert_eq!(
        reasons,
        vec![
            ("http.test".to_owned(), BlockReason::Method),
            ("<invalid>".to_owned(), BlockReason::BadRequest),
            ("tls.test".to_owned(), BlockReason::Method),
            ("other.test".to_owned(), BlockReason::NotAllowed),
            ("grant.test".to_owned(), BlockReason::Method),
        ]
    );
}

#[tokio::test]
async fn plain_denied_host_reason_is_denied() {
    let hosts = [HostTable { host: "http.test", port: None, http: true, private: false }];
    let world = World::new(list_with(&[], &["other.test"], &hosts, &[]));
    let server = Upstream::http().await;
    world.table.add("http.test", &["127.0.0.1"]);
    world.connector.route("127.0.0.1:80", server.addr);
    // The deny list, and the call's own deny set over an http entry.
    let (call, path) = world.call_with(policy(&[], &["http.test"], BUDGET));
    for head in
        ["GET http://other.test/ HTTP/1.1\r\n\r\n", "GET http://http.test/ HTTP/1.1\r\n\r\n"]
    {
        let (status, body) = ask(&path, head).await;
        assert_eq!(status, 403, "{head:?}");
        assert!(body.contains("denied: efr denies this host"), "{body}");
    }
    let reasons: Vec<_> =
        call.blocked().0.iter().map(|record| (record.host.clone(), record.reason)).collect();
    assert_eq!(
        reasons,
        vec![
            ("other.test".to_owned(), BlockReason::Denied),
            ("http.test".to_owned(), BlockReason::Denied),
        ]
    );
    assert!(server.got().is_empty());
}

#[tokio::test]
async fn proxy_plain_refuses_pipelined_request() {
    let (world, server) = http_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let (status, _) = ask(
        &path,
        "GET http://http.test/a HTTP/1.1\r\n\r\nGET http://http.test/b HTTP/1.1\r\n\r\n",
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(server.got(), Vec::<Vec<u8>>::new());
    assert_eq!(call.blocked().0[0].reason, BlockReason::BadRequest);
    // A second request after the answer is never read.
    let mut client = Client::connect(&path).await;
    client.send(b"GET http://http.test/a HTTP/1.1\r\n\r\n").await;
    assert_eq!(client.status().await, 200);
    let _ = client.stream.try_write(b"GET http://http.test/b HTTP/1.1\r\n\r\n");
    assert_eq!(client.rest().await, b"ok");
    let heads = server.got();
    assert_eq!(heads.len(), 1);
    let head = String::from_utf8(heads[0].clone()).unwrap();
    assert!(head.starts_with("GET /a HTTP/1.1\r\n") && !head.contains("/b"), "{head}");
}

#[tokio::test]
async fn proxy_plain_refuses_userinfo_host() {
    let (world, server) = http_world().await;
    let (_call, path) = world.call(&[], BUDGET);
    for head in [
        "GET http://u@http.test/ HTTP/1.1\r\n\r\n",
        "GET http://u:p@http.test/ HTTP/1.1\r\n\r\n",
        "GET http://evil.test@http.test/ HTTP/1.1\r\n\r\n",
        "GET http://http.test%2f@x/ HTTP/1.1\r\n\r\n",
    ] {
        assert_eq!(ask(&path, head).await.0, 400, "{head:?}");
    }
    assert_eq!(server.got(), Vec::<Vec<u8>>::new());
}

#[tokio::test]
async fn proxy_plain_refuses_repeated_headers() {
    let (world, server) = http_world().await;
    let (_call, path) = world.call(&[], BUDGET);
    for header in ["Host: http.test", "Content-Length: 0", "Transfer-Encoding: identity"] {
        let head = format!("GET http://http.test/ HTTP/1.1\r\n{header}\r\n{header}\r\n\r\n");
        assert_eq!(ask(&path, &head).await.0, 400, "{header}");
    }
    assert_eq!(server.got(), Vec::<Vec<u8>>::new());
}

#[tokio::test]
async fn proxy_plain_rebuilds_head() {
    let (world, server) = http_world().await;
    let (call, path) = world.call(&[], BUDGET);
    let head = "GET http://http.test/core.db?x=1 HTTP/1.1\r\nHost: http.test\r\n\
                User-Agent: pacman/7.0\r\nAccept: */*\r\nCookie: session=secret\r\n\
                Proxy-Authorization: Basic dTpw\r\nX-Secret: token\r\nConnection: keep-alive\r\n\
                If-None-Match: \"abc\"\r\n\r\n";
    assert_eq!(ask(&path, head).await.0, 200);
    let heads = server.got();
    assert_eq!(
        String::from_utf8(heads[0].clone()).unwrap(),
        "GET /core.db?x=1 HTTP/1.1\r\nHost: http.test\r\nConnection: close\r\n\
         User-Agent: pacman/7.0\r\nAccept: */*\r\nIf-None-Match: \"abc\"\r\n\r\n"
    );
    let report = call.close(std::time::Duration::from_secs(5)).await;
    assert_eq!(report.bytes_up, u64::try_from(heads[0].len()).unwrap());
    assert_eq!(report.tunnels, 1);
}
