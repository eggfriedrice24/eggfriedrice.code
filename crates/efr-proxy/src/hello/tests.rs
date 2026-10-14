use std::sync::Arc;

use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

/// The ClientHello that rustls writes for `name`, with or without the SNI extension.
/// No server and no network: the bytes go into a `Vec`.
fn rustls_hello(name: &str, sni: bool) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    config.enable_sni = sni;
    let server = rustls::pki_types::ServerName::try_from(name.to_owned()).unwrap();
    let mut connection = rustls::ClientConnection::new(Arc::new(config), server).unwrap();
    let mut bytes = Vec::new();
    while connection.wants_write() {
        connection.write_tls(&mut bytes).unwrap();
    }
    bytes
}

/// The `server_name` extension with these entries: (name type, name).
fn server_name(entries: &[(u8, &str)]) -> (u16, Vec<u8>) {
    let mut list = Vec::new();
    for (kind, name) in entries {
        list.push(*kind);
        list.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
        list.extend_from_slice(name.as_bytes());
    }
    let mut data = u16::try_from(list.len()).unwrap().to_be_bytes().to_vec();
    data.extend(list);
    (0, data)
}

/// A ClientHello body with these extensions, as a fixed byte fixture that does not
/// change with the rustls version.
fn body(extensions: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut body = vec![3, 3];
    body.extend_from_slice(&[7; 32]);
    body.push(0);
    body.extend_from_slice(&[0, 2, 0x13, 0x01]);
    body.extend_from_slice(&[1, 0]);
    let mut block = Vec::new();
    for (kind, data) in extensions {
        block.extend_from_slice(&kind.to_be_bytes());
        block.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
        block.extend_from_slice(data);
    }
    body.extend_from_slice(&u16::try_from(block.len()).unwrap().to_be_bytes());
    body.extend(block);
    body
}

/// A handshake message of type 1 around `body`.
fn handshake(body: &[u8]) -> Vec<u8> {
    let size = u32::try_from(body.len()).unwrap().to_be_bytes();
    let mut message = vec![1, size[1], size[2], size[3]];
    message.extend_from_slice(body);
    message
}

/// `message` in handshake records of at most `size` bytes each.
fn records(message: &[u8], size: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for fragment in message.chunks(size) {
        bytes.extend_from_slice(&[22, 3, 1]);
        bytes.extend_from_slice(&u16::try_from(fragment.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(fragment);
    }
    bytes
}

fn fixture(extensions: &[(u16, Vec<u8>)]) -> Vec<u8> {
    records(&handshake(&body(extensions)), 16384)
}

fn sni_of(bytes: &[u8]) -> Option<String> {
    match parse(bytes) {
        Hello::Done { sni, ech: false, used } => {
            assert_eq!(used, bytes.len());
            sni
        }
        other => panic!("not a plain hello: {other:?}"),
    }
}

#[test]
fn a_rustls_hello_gives_its_server_name() {
    let bytes = rustls_hello("allowed.test", true);
    assert_eq!(sni_of(&bytes).as_deref(), Some("allowed.test"));
}

#[test]
fn a_rustls_hello_without_sni_gives_none() {
    assert_eq!(sni_of(&rustls_hello("allowed.test", false)), None);
    // rustls sends no server name for an address.
    assert_eq!(sni_of(&rustls_hello("192.0.2.1", true)), None);
}

#[test]
fn a_fixture_gives_its_server_name_as_written() {
    let bytes = fixture(&[server_name(&[(0, "Index.Crates.IO.")])]);
    assert_eq!(sni_of(&bytes).as_deref(), Some("Index.Crates.IO."));
    assert_eq!(sni_of(&fixture(&[])), None);
    assert_eq!(sni_of(&records(&handshake(&body(&[])[..41]), 16384)), None);
    // A name type that is not host_name is no server name.
    assert_eq!(sni_of(&fixture(&[server_name(&[(7, "x")])])), None);
}

#[test]
fn ech_and_esni_extensions_are_marked() {
    for kind in [0xfe08, 0xfe0a, 0xfe0d, 0xffce] {
        let bytes = fixture(&[server_name(&[(0, "a.test")]), (kind, vec![1, 2, 3])]);
        assert_eq!(
            parse(&bytes),
            Hello::Done { sni: Some("a.test".to_owned()), ech: true, used: bytes.len() },
            "extension {kind:#06x}"
        );
    }
    for kind in [0xfe07, 0xfe0e, 0xffcd, 0x0015] {
        assert!(!is_ech(kind), "{kind:#06x}");
    }
}

#[test]
fn a_hello_split_over_records_gives_the_same_name() {
    let whole = rustls_hello("allowed.test", true);
    let message = &whole[5..];
    for size in [1, 2, 3, 4, 5, 100, 512] {
        assert_eq!(sni_of(&records(message, size)).as_deref(), Some("allowed.test"), "{size}");
    }
}

#[test]
fn every_prefix_of_a_hello_is_partial() {
    let whole = records(&rustls_hello("allowed.test", true)[5..], 300);
    for end in 0..whole.len() {
        assert_eq!(parse(&whole[..end]), Hello::Partial, "prefix of {end} bytes");
    }
}

#[test]
fn bytes_after_the_hello_stay_for_the_caller() {
    let hello = fixture(&[server_name(&[(0, "a.test")])]);
    let mut bytes = hello.clone();
    bytes.extend_from_slice(&[23, 3, 3, 0, 1, 0]);
    assert_eq!(
        parse(&bytes),
        Hello::Done { sni: Some("a.test".to_owned()), ech: false, used: hello.len() }
    );
}

#[test]
fn bytes_that_are_no_tls_are_cut_at_once() {
    for bytes in [
        &b"GET / HTTP/1.1\r\n\r\n"[..],
        b"SSH-2.0-OpenSSH_9.9\r\n",
        b"\x17\x03\x03\x00\x01\x00",
        b"\x16\x02",
        b"\x16\x03\x05",
        b"\x16\x03\x01\x00\x00",
        b"\x16\x03\x01\x40\x01",
    ] {
        assert_eq!(parse(bytes), Hello::NotTls, "{bytes:?}");
    }
    // A later record of another type before the hello is whole.
    let message = handshake(&body(&[server_name(&[(0, "a.test")])]));
    let mut bytes = records(&message[..10], 16384);
    bytes.extend_from_slice(&[23, 3, 3, 0, 1, 0]);
    assert_eq!(parse(&bytes), Hello::NotTls);
}

#[test]
fn a_handshake_that_is_no_client_hello_is_not_hello() {
    assert_eq!(parse(b"\x16\x03\x01\x00\x05hello"), Hello::NotHello);
    assert_eq!(parse(b"\x16\x03\x03\x00\x01\x02"), Hello::NotHello);
}

#[test]
fn a_malformed_hello_is_not_tls() {
    let name = server_name(&[(0, "a.test")]);
    let cases: Vec<Vec<u8>> = vec![
        // The extensions block says more than it holds.
        {
            let mut bytes = body(std::slice::from_ref(&name));
            let at = 2 + 32 + 1 + 4 + 2;
            bytes[at + 1] += 1;
            bytes
        },
        // The same extension twice.
        body(&[name.clone(), name.clone()]),
        // Two host names in one extension.
        body(&[server_name(&[(0, "a.test"), (0, "b.test")])]),
        // An empty host name, and one with a space.
        body(&[server_name(&[(0, "")])]),
        body(&[server_name(&[(0, "a test")])]),
        // Bytes after the extensions.
        {
            let mut bytes = body(std::slice::from_ref(&name));
            bytes.push(0);
            bytes
        },
        // A session id over 32 bytes.
        {
            let mut bytes = body(&[]);
            bytes[34] = 33;
            bytes
        },
        // No cipher suite.
        {
            let mut bytes = body(&[]);
            bytes[36] = 0;
            bytes
        },
        // A legacy version that is no TLS version.
        {
            let mut bytes = body(&[]);
            bytes[0] = 2;
            bytes
        },
    ];
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(parse(&records(&handshake(case), 16384)), Hello::NotTls, "case {index}");
    }
}

#[test]
fn rustls_sends_no_ech_by_default() {
    for sni in [true, false] {
        let bytes = rustls_hello("allowed.test", sni);
        assert!(matches!(parse(&bytes), Hello::Done { ech: false, .. }), "sni {sni}");
    }
}

proptest! {
    #[test]
    fn hello_parse_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let parsed = parse(&bytes);
        if let Hello::Done { used, .. } = parsed {
            prop_assert!(used <= bytes.len());
        }
    }

    #[test]
    fn hello_parse_never_panics_on_a_changed_hello(at in 0usize..400, byte in any::<u8>()) {
        let mut bytes = fixture(&[server_name(&[(0, "allowed.test")]), (0x0015, vec![0; 200])]);
        let at = at % bytes.len();
        bytes[at] = byte;
        let _ = parse(&bytes);
        let _ = parse(&bytes[..at]);
    }

    #[test]
    fn hello_parse_split_equals_whole(
        sizes in proptest::collection::vec(1usize..400, 1..40),
        name in "[a-z]{1,20}\\.test",
    ) {
        let message = handshake(&body(&[server_name(&[(0, &name)]), (0x0015, vec![0; 300])]));
        let mut bytes = Vec::new();
        let mut rest = &message[..];
        let mut sizes = sizes.iter().cycle();
        while !rest.is_empty() {
            let size = (*sizes.next().unwrap()).min(rest.len());
            let (fragment, after) = rest.split_at(size);
            bytes.extend(records(fragment, size));
            rest = after;
        }
        prop_assert_eq!(
            parse(&bytes),
            Hello::Done { sni: Some(name), ech: false, used: bytes.len() }
        );
    }
}
