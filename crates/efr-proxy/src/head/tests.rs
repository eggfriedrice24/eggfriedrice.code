use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

const LIMIT: usize = 8 * 1024;

fn done(text: &str) -> Request {
    match parse(text.as_bytes(), LIMIT) {
        Parsed::Done { value, used } => {
            assert_eq!(used, text.len(), "the head of {text:?} used every byte");
            value
        }
        other => panic!("{text:?} gave {other:?}"),
    }
}

fn refused(text: &str) {
    assert_eq!(
        parse(text.as_bytes(), LIMIT),
        Parsed::Refused(BlockReason::BadRequest),
        "{text:?} must be refused"
    );
}

fn connect(host: &str, port: u16) -> Request {
    Request::Connect { host: host.to_owned(), port }
}

fn get(host: &str, port: u16, target: &str) -> Request {
    Request::Plain {
        method: Method::Get,
        host: host.to_owned(),
        port,
        target: target.to_owned(),
        headers: Vec::new(),
    }
}

#[test]
fn connect_reads_the_authority_form() {
    assert_eq!(done("CONNECT a.test:443 HTTP/1.1\r\n\r\n"), connect("a.test", 443));
    assert_eq!(
        done("CONNECT A.Test.:8443 HTTP/1.1\r\nHost: a.test:8443\r\n\r\n"),
        connect("a.test", 8443)
    );
    assert_eq!(done("CONNECT [2001:DB8::1]:443 HTTP/1.1\r\n\r\n"), connect("2001:db8::1", 443));
    assert_eq!(done("CONNECT 192.0.2.1:443 HTTP/1.0\r\n\r\n"), connect("192.0.2.1", 443));
    assert_eq!(
        done("CONNECT a.test:443 HTTP/1.1\r\nHost: a.test\r\nUser-Agent: curl\r\n\r\n"),
        connect("a.test", 443)
    );
}

#[test]
fn connect_refuses_a_bad_target() {
    for target in [
        "a.test",
        "a.test:",
        "a.test:+443",
        "a.test:0",
        "a.test:65536",
        "a.test:0443x",
        "u@a.test:443",
        "a.test:443/",
        "a%41.test:443",
        "[a.test]:443",
        "[2001:db8::1%eth0]:443",
        "a..test:443",
        "-",
        "http://a.test:443",
        "*.test:443",
    ] {
        refused(&format!("CONNECT {target} HTTP/1.1\r\n\r\n"));
    }
}

#[test]
fn connect_refuses_another_host_header() {
    refused("CONNECT a.test:443 HTTP/1.1\r\nHost: b.test:443\r\n\r\n");
    refused("CONNECT a.test:443 HTTP/1.1\r\nHost: a.test:444\r\n\r\n");
    refused("CONNECT a.test:443 HTTP/1.1\r\nHost: a.test\r\nHost: a.test\r\n\r\n");
}

#[test]
fn connect_has_no_body() {
    refused("CONNECT a.test:443 HTTP/1.1\r\nContent-Length: 5\r\n\r\n");
    refused("CONNECT a.test:443 HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert_eq!(
        done("CONNECT a.test:443 HTTP/1.1\r\nContent-Length: 0\r\n\r\n"),
        connect("a.test", 443)
    );
}

#[test]
fn the_request_line_is_strict() {
    for head in [
        "CONNECT  a.test:443 HTTP/1.1\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1 \r\n\r\n",
        "CONNECT a.test:443\r\n\r\n",
        "connect a.test:443 HTTP/1.1\r\n\r\n",
        "CONNECT a.test:443 HTTP/2\r\n\r\n",
        "CONNECT a.test:443 http/1.1\r\n\r\n",
        "\r\nCONNECT a.test:443 HTTP/1.1\r\n\r\n",
        "ABCDEFGHIJKLMNOPQ http://a.test/ HTTP/1.1\r\n\r\n",
    ] {
        refused(head);
    }
}

#[test]
fn control_characters_are_refused_at_once() {
    for head in [
        "CONNECT a\u{1}.test:443 HTTP/1.1\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\nUser-Agent: a\u{1b}[31m\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\nUser-Agent:\ta\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\nHost: a.test\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\rHost: a.test\r\n\r\n",
        "CONNECT a.test:443 HTTP/1.1\r\nX: \u{7f}\r\n\r\n",
        "CONNECT a.t\u{e9}st:443 HTTP/1.1\r\n\r\n",
    ] {
        refused(head);
    }
    // No end of the head yet, but a refusal already.
    assert_eq!(
        parse(b"CONNECT a\x00", LIMIT),
        Parsed::Refused(BlockReason::BadRequest),
        "a NUL refuses before the end of the head"
    );
}

#[test]
fn a_head_without_its_end_is_partial_up_to_the_limit() {
    assert_eq!(parse(b"", LIMIT), Parsed::Partial);
    assert_eq!(parse(b"CONNECT a.test:443 HTTP/1.1\r\n", LIMIT), Parsed::Partial);
    assert_eq!(parse(b"CONNECT a.test:443 HTTP/1.1\r\n\r", LIMIT), Parsed::Partial);
    let long = format!("CONNECT a.test:443 HTTP/1.1\r\nUser-Agent: {}", "x".repeat(LIMIT));
    assert_eq!(parse(long.as_bytes(), LIMIT), Parsed::Refused(BlockReason::BadRequest));
    // The end of the head after the limit does not count.
    let head = "CONNECT a.test:443 HTTP/1.1\r\n\r\n";
    assert_eq!(parse(head.as_bytes(), head.len() - 1), Parsed::Refused(BlockReason::BadRequest));
    assert_eq!(
        parse(head.as_bytes(), head.len()),
        Parsed::Done { value: connect("a.test", 443), used: head.len() }
    );
}

#[test]
fn bytes_after_the_head_stay_for_the_caller() {
    let bytes = b"CONNECT a.test:443 HTTP/1.1\r\n\r\n\x16\x03\x01\x00\x05\x01\xff\xff";
    assert_eq!(parse(bytes, LIMIT), Parsed::Done { value: connect("a.test", 443), used: 31 });
}

#[test]
fn plain_reads_the_absolute_form() {
    assert_eq!(done("GET http://a.test/ HTTP/1.1\r\n\r\n"), get("a.test", 80, "/"));
    assert_eq!(done("GET http://a.test HTTP/1.1\r\n\r\n"), get("a.test", 80, "/"));
    assert_eq!(
        done("GET HTTP://A.test:8080/x?y=1 HTTP/1.1\r\n\r\n"),
        get("a.test", 8080, "/x?y=1")
    );
    assert_eq!(done("GET http://a.test?y HTTP/1.1\r\n\r\n"), get("a.test", 80, "/?y"));
    assert_eq!(
        done("GET http://a.test/core.db HTTP/1.1\r\nHost: a.test:80\r\n\r\n"),
        get("a.test", 80, "/core.db")
    );
    assert_eq!(done("GET http://[2001:db8::5]/ HTTP/1.1\r\n\r\n"), get("2001:db8::5", 80, "/"));
    let head = done("HEAD http://a.test/ HTTP/1.1\r\n\r\n");
    assert!(matches!(head, Request::Plain { method: Method::Head, .. }), "{head:?}");
}

#[test]
fn plain_keeps_only_the_allowed_headers() {
    let request = done(
        "GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\nuser-agent: pacman/7\r\n\
         Proxy-Authorization: Basic eDp5\r\nCookie: a=b\r\nAccept: */*\r\n\
         Connection: keep-alive\r\nRANGE: bytes=0-1\r\n\r\n",
    );
    let Request::Plain { headers, .. } = &request else { panic!("{request:?}") };
    assert_eq!(
        headers,
        &vec![
            ("User-Agent".to_owned(), "pacman/7".to_owned()),
            ("Accept".to_owned(), "*/*".to_owned()),
            ("Range".to_owned(), "bytes=0-1".to_owned()),
        ]
    );
    assert_eq!(
        String::from_utf8(upstream_head(&request)).unwrap(),
        "GET / HTTP/1.1\r\nHost: a.test\r\nConnection: close\r\nUser-Agent: pacman/7\r\n\
         Accept: */*\r\nRange: bytes=0-1\r\n\r\n"
    );
}

#[test]
fn the_upstream_head_names_the_port_and_brackets() {
    let request = done("HEAD http://[2001:db8::5]:8080/a HTTP/1.1\r\n\r\n");
    assert_eq!(
        String::from_utf8(upstream_head(&request)).unwrap(),
        "HEAD /a HTTP/1.1\r\nHost: [2001:db8::5]:8080\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(upstream_head(&connect("a.test", 443)), Vec::<u8>::new());
}

#[test]
fn plain_refuses_uri_tricks_and_bad_headers() {
    for head in [
        "GET / HTTP/1.1\r\nHost: a.test\r\n\r\n",
        "GET https://a.test/ HTTP/1.1\r\n\r\n",
        "GET http://u@a.test/ HTTP/1.1\r\n\r\n",
        "GET http://u:p@a.test/ HTTP/1.1\r\n\r\n",
        "GET http://a%2etest/ HTTP/1.1\r\n\r\n",
        "GET http://[a.test]/ HTTP/1.1\r\n\r\n",
        "GET http://a.test/#top HTTP/1.1\r\n\r\n",
        "GET http:///x HTTP/1.1\r\n\r\n",
        "GET http://a.test:/ HTTP/1.1\r\n\r\n",
        "GET http://a.test:81/ HTTP/1.1\r\nHost: a.test\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nHost: a.test:81\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nHost: b.test\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\nHost: a.test\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nUser-Agent: a\r\n b\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nUser Agent: a\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\n: a\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nNo-Colon\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nContent-Length: 3\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        "GET http://a.test/ HTTP/1.1\r\nTransfer-Encoding: x\r\nTransfer-Encoding: y\r\n\r\n",
        "HEAD http://a.test/ HTTP/1.1\r\nContent-Length: 1\r\n\r\n",
    ] {
        refused(head);
    }
}

#[test]
fn other_methods_parse_for_their_405() {
    let request = done("POST http://a.test/ HTTP/1.1\r\nContent-Length: 3\r\n\r\n");
    assert!(
        matches!(&request, Request::Plain { method: Method::Other(method), .. } if method == "POST"),
        "{request:?}"
    );
    let request = done("PUT http://a.test/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert!(matches!(&request, Request::Plain { method: Method::Other(_), .. }), "{request:?}");
}

/// A valid host name of one to three labels below `.test`.
fn host() -> impl Strategy<Value = String> {
    proptest::collection::vec("[a-z0-9]{1,8}", 1..3).prop_map(|labels| labels.join(".") + ".test")
}

proptest! {
    #[test]
    fn head_parse_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), limit in 0usize..600) {
        let parsed = parse(&bytes, limit);
        if let Parsed::Done { used, .. } = parsed {
            prop_assert!(used <= bytes.len().min(limit));
        }
    }

    #[test]
    fn head_parse_never_panics_near_a_head(
        start in prop::sample::select(vec![
            "CONNECT a.test:443 HTTP/1.1\r\n",
            "GET http://a.test/ HTTP/1.1\r\n",
            "HEAD http://[::1]:8/ HTTP/1.1\r\nHost: [::1]:8\r\n",
        ]),
        tail in "[ -~\r\n]{0,64}",
    ) {
        let text = format!("{start}{tail}");
        let _ = parse(text.as_bytes(), LIMIT);
    }

    #[test]
    fn head_parse_refuses_uri_tricks(
        host in host(),
        user in "[a-z]{1,6}",
        fragment in "[a-z]{0,6}",
        value in "[a-z]{1,6}",
    ) {
        for head in [
            format!("GET http://{user}@{host}/ HTTP/1.1\r\n\r\n"),
            format!("GET http://{user}:x@{host}/ HTTP/1.1\r\n\r\n"),
            format!("GET http://{host}%2e{user}/ HTTP/1.1\r\n\r\n"),
            format!("GET http://{host}/#{fragment} HTTP/1.1\r\n\r\n"),
            format!("GET http://{host}/ HTTP/1.1\r\nAccept: {value}\r\n {value}\r\n\r\n"),
            format!("GET http://{host}/ HTTP/1.1\r\nAccept: {value}\r\n\t{value}\r\n\r\n"),
            format!("CONNECT {user}@{host}:443 HTTP/1.1\r\n\r\n"),
            format!("CONNECT {host}%{user}:443 HTTP/1.1\r\n\r\n"),
        ] {
            prop_assert_eq!(parse(head.as_bytes(), LIMIT), Parsed::Refused(BlockReason::BadRequest), "{}", head);
        }
        let plain = format!("GET http://{host}/ HTTP/1.1\r\nHost: {host}\r\n\r\n");
        let parsed = parse(plain.as_bytes(), LIMIT);
        prop_assert!(matches!(parsed, Parsed::Done { .. }), "{:?}", parsed);
    }
}
