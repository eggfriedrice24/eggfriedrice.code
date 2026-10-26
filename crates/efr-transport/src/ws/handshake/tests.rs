use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{
    MAX_HEAD_BYTES, MAX_HEADERS, Parsed, Upgrade, WebSocketRefusal, accept_response, accept_value,
    parse, refusal_response,
};

const ORIGIN: &str = "http://100.64.0.1:7878";

/// The key and accept value of RFC 6455, section 1.3.
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

fn origins() -> Vec<String> {
    vec![ORIGIN.to_owned()]
}

/// The header lines of a valid upgrade, as SocketRocket sends them.
fn good_headers() -> Vec<String> {
    vec![
        "Host: 100.64.0.1:7878".to_owned(),
        "Upgrade: websocket".to_owned(),
        "Connection: Upgrade".to_owned(),
        format!("Sec-WebSocket-Key: {KEY}"),
        "Sec-WebSocket-Version: 13".to_owned(),
        format!("Origin: {ORIGIN}"),
    ]
}

fn head_with(request_line: &str, headers: &[String]) -> Vec<u8> {
    let mut head = format!("{request_line}\r\n");
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head.into_bytes()
}

fn head(headers: &[String]) -> Vec<u8> {
    head_with("GET / HTTP/1.1", headers)
}

/// The headers of a valid upgrade with the header `name` replaced by `lines`.
fn replacing(name: &str, lines: &[&str]) -> Vec<String> {
    let mut headers: Vec<String> = good_headers()
        .into_iter()
        .filter(|line| {
            !line.to_ascii_lowercase().starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
        .collect();
    headers.extend(lines.iter().map(|line| (*line).to_owned()));
    headers
}

fn refusal(bytes: &[u8]) -> WebSocketRefusal {
    match parse(bytes, &origins()) {
        Err(refusal) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn accepted(bytes: &[u8]) -> Upgrade {
    match parse(bytes, &origins()) {
        Ok(Parsed::Complete(upgrade)) => upgrade,
        other => panic!("expected an upgrade, got {other:?}"),
    }
}

#[test]
fn the_accept_value_matches_the_example_of_the_rfc() {
    assert_eq!(accept_value(KEY), ACCEPT);
}

#[test]
fn a_valid_upgrade_gives_the_accept_value_and_the_head_length() {
    let mut bytes = head(&good_headers());
    let head_len = bytes.len();
    bytes.extend_from_slice(b"\x81\x80first frame");
    assert_eq!(accepted(&bytes), Upgrade { head_len, accept: ACCEPT.to_owned() });
}

#[test]
fn a_missing_origin_is_accepted() {
    accepted(&head(&replacing("origin", &[])));
}

#[test]
fn every_origin_but_an_allowed_one_is_refused() {
    for origin in [
        "null",
        "",
        "http://100.64.0.1:7879",
        "https://100.64.0.1:7878",
        "http://100.64.0.2:7878",
        "HTTP://100.64.0.1:7878",
        "http://100.64.0.1:7878/",
        "http://evil.example",
    ] {
        let line = format!("Origin: {origin}");
        assert_eq!(
            refusal(&head(&replacing("origin", &[&line]))),
            WebSocketRefusal::Origin,
            "{origin:?}"
        );
    }
}

#[test]
fn an_origin_is_refused_when_none_is_allowed() {
    assert_eq!(parse(&head(&good_headers()), &[]), Err(WebSocketRefusal::Origin));
}

#[test]
fn a_second_allowed_origin_is_accepted() {
    let allowed = vec![ORIGIN.to_owned(), "http://efr.tail1234.ts.net:7878".to_owned()];
    let headers = replacing("origin", &["Origin: http://efr.tail1234.ts.net:7878"]);
    assert!(matches!(parse(&head(&headers), &allowed), Ok(Parsed::Complete(_))));
}

#[test]
fn a_repeated_origin_is_malformed() {
    let line = format!("Origin: {ORIGIN}");
    assert_eq!(refusal(&head(&replacing("origin", &[&line, &line]))), WebSocketRefusal::Malformed);
}

#[test]
fn only_get_of_the_root_over_http_1_1_is_served() {
    let cases = [
        ("POST / HTTP/1.1", WebSocketRefusal::Method),
        ("get / HTTP/1.1", WebSocketRefusal::Method),
        ("GET /ws HTTP/1.1", WebSocketRefusal::Path),
        ("GET /?token=secret HTTP/1.1", WebSocketRefusal::Path),
        ("GET http://100.64.0.1:7878/ HTTP/1.1", WebSocketRefusal::Path),
        ("GET / HTTP/1.0", WebSocketRefusal::HttpVersion),
    ];
    for (line, expected) in cases {
        assert_eq!(refusal(&head_with(line, &good_headers())), expected, "{line}");
    }
}

#[test]
fn a_request_without_the_upgrade_headers_is_refused() {
    assert_eq!(refusal(&head(&replacing("upgrade", &[]))), WebSocketRefusal::NotUpgrade);
    assert_eq!(
        refusal(&head(&replacing("upgrade", &["Upgrade: h2c"]))),
        WebSocketRefusal::NotUpgrade
    );
    assert_eq!(refusal(&head(&replacing("connection", &[]))), WebSocketRefusal::NotUpgrade);
    let keep_alive = replacing("connection", &["Connection: keep-alive"]);
    assert_eq!(refusal(&head(&keep_alive)), WebSocketRefusal::NotUpgrade);
}

#[test]
fn the_upgrade_tokens_may_be_in_lists_and_in_any_case() {
    let headers =
        replacing("connection", &["Connection: keep-alive", "connection: KEEP-ALIVE, UpGrade"]);
    let headers: Vec<String> =
        headers
            .into_iter()
            .map(|line| {
                if line.starts_with("Upgrade") {
                    "UPGRADE: h2c, WebSocket".to_owned()
                } else {
                    line
                }
            })
            .collect();
    accepted(&head(&headers));
}

#[test]
fn only_websocket_version_13_is_served() {
    for version in ["8", "13, 8", "14", ""] {
        let line = format!("Sec-WebSocket-Version: {version}");
        assert_eq!(
            refusal(&head(&replacing("sec-websocket-version", &[&line]))),
            WebSocketRefusal::WebSocketVersion,
            "{version:?}"
        );
    }
    let missing = replacing("sec-websocket-version", &[]);
    assert_eq!(refusal(&head(&missing)), WebSocketRefusal::Malformed);
}

#[test]
fn the_key_must_be_sixteen_bytes_in_base64_and_come_once() {
    for key in ["not base64!", "AAAAAAAAAAAAAAAAAAAA", "dGhlIHNhbXBsZSBub25jZQ", ""] {
        let line = format!("Sec-WebSocket-Key: {key}");
        assert_eq!(
            refusal(&head(&replacing("sec-websocket-key", &[&line]))),
            WebSocketRefusal::Key,
            "{key:?}"
        );
    }
    assert_eq!(refusal(&head(&replacing("sec-websocket-key", &[]))), WebSocketRefusal::Malformed);
    let line = format!("Sec-WebSocket-Key: {KEY}");
    let twice = replacing("sec-websocket-key", &[&line, &line]);
    assert_eq!(refusal(&head(&twice)), WebSocketRefusal::Malformed);
}

#[test]
fn a_host_must_come_once() {
    assert_eq!(refusal(&head(&replacing("host", &[]))), WebSocketRefusal::Malformed);
    let twice = replacing("host", &["Host: a", "Host: b"]);
    assert_eq!(refusal(&head(&twice)), WebSocketRefusal::Malformed);
}

#[test]
fn a_request_with_a_body_is_refused() {
    let mut headers = good_headers();
    headers.push("Content-Length: 0".to_owned());
    accepted(&head(&headers));
    for line in ["Content-Length: 5", "Transfer-Encoding: chunked"] {
        let mut headers = good_headers();
        headers.push(line.to_owned());
        assert_eq!(refusal(&head(&headers)), WebSocketRefusal::Body, "{line}");
    }
}

#[test]
fn a_cookie_and_other_headers_are_never_read() {
    let mut bytes =
        b"GET / HTTP/1.1\r\nCookie: session=\xff\xfe\r\nAuthorization: Basic eA==\r\n".to_vec();
    for line in good_headers() {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"\r\n");
    accepted(&bytes);
}

#[test]
fn a_header_value_that_is_not_utf8_is_malformed_when_the_handshake_reads_it() {
    let mut bytes = b"GET / HTTP/1.1\r\nOrigin: http://\xff\r\n".to_vec();
    for line in replacing("origin", &[]) {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"\r\n");
    assert_eq!(refusal(&bytes), WebSocketRefusal::Malformed);
}

#[test]
fn garbage_is_malformed() {
    assert_eq!(
        refusal(b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03"),
        WebSocketRefusal::Malformed
    );
    assert_eq!(refusal(b"GET / HTTP/1.1\r\nno colon here\r\n\r\n"), WebSocketRefusal::Malformed);
}

/// A valid head padded with one header to exactly `len` bytes.
fn head_of_len(len: usize) -> Vec<u8> {
    let base = head(&good_headers()).len();
    // "X-Pad: " plus the value plus CRLF.
    let pad = len - base - "X-Pad: \r\n".len();
    let mut headers = good_headers();
    headers.push(format!("X-Pad: {}", "a".repeat(pad)));
    let bytes = head(&headers);
    assert_eq!(bytes.len(), len);
    bytes
}

#[test]
fn a_head_of_8_kib_is_accepted_and_one_byte_more_is_too_large() {
    accepted(&head_of_len(MAX_HEAD_BYTES));
    let bigger = head_of_len(MAX_HEAD_BYTES + 1);
    assert_eq!(refusal(&bigger), WebSocketRefusal::HeadTooLarge);
    // The reader stops at 8 KiB, so a head that is still partial there is too large.
    assert_eq!(refusal(&bigger[..MAX_HEAD_BYTES]), WebSocketRefusal::HeadTooLarge);
}

fn headers_counting(count: usize) -> Vec<String> {
    let mut headers = good_headers();
    let extra = count - headers.len();
    headers.extend((0..extra).map(|n| format!("X-Extra-{n}: {n}")));
    headers
}

#[test]
fn thirty_two_headers_are_accepted_and_thirty_three_are_too_many() {
    accepted(&head(&headers_counting(MAX_HEADERS)));
    assert_eq!(
        refusal(&head(&headers_counting(MAX_HEADERS + 1))),
        WebSocketRefusal::TooManyHeaders
    );
}

#[test]
fn every_refusal_is_a_plain_http_error_that_closes() {
    let all = [
        WebSocketRefusal::HeadTooLarge,
        WebSocketRefusal::TooManyHeaders,
        WebSocketRefusal::Malformed,
        WebSocketRefusal::HttpVersion,
        WebSocketRefusal::Method,
        WebSocketRefusal::Path,
        WebSocketRefusal::Body,
        WebSocketRefusal::NotUpgrade,
        WebSocketRefusal::WebSocketVersion,
        WebSocketRefusal::Key,
        WebSocketRefusal::Origin,
    ];
    for refusal in all {
        let bytes = refusal_response(refusal);
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut response = httparse::Response::new(&mut headers);
        let httparse::Status::Complete(head_len) = response.parse(&bytes).unwrap() else {
            panic!("{refusal:?}: the response head is not complete");
        };
        let code = response.code.unwrap();
        assert_eq!(code, refusal.status(), "{refusal:?}");
        assert!((400..600).contains(&code), "{refusal:?}");
        let header = |name: &str| {
            response
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case(name))
                .map(|header| std::str::from_utf8(header.value).unwrap().to_owned())
        };
        let body = &bytes[head_len..];
        assert_eq!(header("content-length"), Some(body.len().to_string()), "{refusal:?}");
        assert_eq!(header("connection").as_deref(), Some("close"), "{refusal:?}");
        assert_eq!(body, format!("{refusal}\n").as_bytes(), "{refusal:?}");
    }
}

#[test]
fn the_extra_headers_of_a_refusal_follow_the_http_rules() {
    let text = |refusal| String::from_utf8(refusal_response(refusal)).unwrap();
    assert!(text(WebSocketRefusal::Method).contains("\r\nAllow: GET\r\n"));
    assert!(text(WebSocketRefusal::NotUpgrade).contains("\r\nUpgrade: websocket\r\n"));
    assert!(text(WebSocketRefusal::WebSocketVersion).contains("\r\nSec-WebSocket-Version: 13\r\n"));
}

#[test]
fn the_accept_response_switches_protocols_with_the_accept_value() {
    let upgrade = Upgrade { head_len: 0, accept: ACCEPT.to_owned() };
    let text = String::from_utf8(accept_response(&upgrade)).unwrap();
    assert_eq!(
        text,
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {ACCEPT}\r\n\r\n"
        )
    );
}

proptest! {
    #[test]
    fn every_strict_prefix_of_a_valid_head_is_partial(cut in 0..head(&good_headers()).len()) {
        let bytes = head(&good_headers());
        prop_assert_eq!(parse(&bytes[..cut], &origins()), Ok(Parsed::Partial));
    }

    #[test]
    fn the_order_and_the_case_of_the_headers_do_not_matter(
        order in Just((0..6).collect::<Vec<usize>>()).prop_shuffle(),
        upper in proptest::collection::vec(any::<bool>(), 6),
    ) {
        let lines = good_headers();
        let headers: Vec<String> = order
            .iter()
            .zip(&upper)
            .map(|(&index, &upper)| {
                let (name, value) = lines[index].split_once(':').unwrap();
                let name = if upper { name.to_ascii_uppercase() } else { name.to_ascii_lowercase() };
                format!("{name}:{value}")
            })
            .collect();
        let upgrade = accepted(&head(&headers));
        prop_assert_eq!(upgrade.accept, ACCEPT);
    }

    #[test]
    fn arbitrary_bytes_never_panic_and_never_pass_without_the_upgrade_headers(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let result = parse(&bytes, &origins());
        if let Ok(Parsed::Complete(upgrade)) = result {
            prop_assert!(upgrade.head_len <= bytes.len());
            let text = String::from_utf8_lossy(&bytes[..upgrade.head_len]).to_ascii_lowercase();
            prop_assert!(text.contains("sec-websocket-key"));
        }
    }

    #[test]
    fn a_valid_head_followed_by_any_bytes_is_accepted_with_the_same_length(
        tail in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut bytes = head(&good_headers());
        let head_len = bytes.len();
        bytes.extend_from_slice(&tail);
        prop_assert_eq!(accepted(&bytes).head_len, head_len);
    }
}
