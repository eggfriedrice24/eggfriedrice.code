//! The text of the shell tool for efr's network proxy (phase D2, DP28).

use efr_protocol::{BlockReason, Blocked};
use pretty_assertions::assert_eq;

use super::super::{NEEDS_MEMBERS, blocked_note};
use crate::Tool as _;

fn blocked(host: &str, port: u16, reason: BlockReason) -> Blocked {
    serde_json::from_value(serde_json::json!({ "host": host, "port": port, "reason": reason }))
        .unwrap()
}

#[test]
fn blocked_note_one_sentence_per_reason() {
    let entries = [
        blocked("a.test", 443, BlockReason::NotAllowed),
        blocked("b.test", 443, BlockReason::NotAllowed),
        blocked("lan.test", 443, BlockReason::PrivateAddress),
        blocked("a.test", 443, BlockReason::NotAllowed),
        blocked("up.test", 443, BlockReason::UploadLimit),
        blocked("plain.test", 80, BlockReason::Method),
        blocked("front.test", 443, BlockReason::SniMismatch),
        blocked("chrome.test", 443, BlockReason::NotTls),
        blocked("192.0.2.1", 443, BlockReason::IpLiteral),
        blocked("<invalid>", 443, BlockReason::BadRequest),
        blocked("many.test", 443, BlockReason::Limit),
    ];
    let note = blocked_note(&entries, 2, 4_194_304).unwrap();
    let lines: Vec<&str> = note.lines().collect();
    assert_eq!(
        lines,
        [
            "[efr: the network proxy refused a.test:443, b.test:443: not on the allow list. To \
             reach a host, call shell again with needs.hosts.]",
            "[efr: the network proxy refused lan.test:443: it resolves to a local or private \
             address. A host grant does not help.]",
            "[efr: the network proxy cut up.test:443: the command sent more than 4194304 \
             bytes, the upload limit of a call (network.max_upload_bytes).]",
            "[efr: the network proxy refused plain.test:80: plain http allows only GET and \
             HEAD.]",
            "[efr: the network proxy refused front.test:443: the TLS server name was not the \
             host of the connection. Do not try another way.]",
            "[efr: the network proxy refused chrome.test:443: the connection sent no TLS hello \
             with a server name, or it asked for Encrypted Client Hello. Do not try another \
             way.]",
            "[efr: the network proxy refused 192.0.2.1:443: the command named an IP address. \
             Do not try another way.]",
            "[efr: the network proxy refused <invalid>:443: the request was malformed.]",
            "[efr: the network proxy refused many.test:443: the command had too many open \
             connections.]",
            "[efr: the network proxy also refused 2 more connections.]",
        ]
    );
    assert_eq!(blocked_note(&[], 0, 0), None);
    assert_eq!(
        blocked_note(&[], 1, 0).as_deref(),
        Some("[efr: the network proxy also refused 1 more connection.]")
    );
    // Without the number, the upload note names the key.
    let note = blocked_note(&[blocked("up.test", 443, BlockReason::UploadLimit)], 0, 0).unwrap();
    assert!(note.contains("the upload limit of a call (network.max_upload_bytes)"), "{note}");
    // A reason of a newer efrd still gets a sentence.
    let unknown: Blocked = serde_json::from_value(
        serde_json::json!({ "host": "x.test", "port": 443, "reason": "from_the_future" }),
    )
    .unwrap();
    assert_eq!(
        blocked_note(&[unknown], 0, 0).as_deref(),
        Some(
            "[efr: the network proxy refused x.test:443: for a reason that this build does not know.]"
        )
    );
}

#[test]
fn hosts_member_text_holds_in_both_modes() {
    let (_, text) = NEEDS_MEMBERS.iter().find(|(name, _)| *name == "hosts").unwrap();
    // NOTE: the tool spec is static and part of the cache prefix, so the text must hold
    // with the proxy and without it: it names no allow list and promises no proxy.
    assert!(text.contains("localhost"), "{text}");
    assert!(text.contains("HTTPS_PROXY"), "{text}");
    for word in ["allow list", "efr's proxy", "phase"] {
        assert!(!text.contains(word), "{word:?} in {text}");
    }
    let spec = super::tool().spec();
    let description = spec.description;
    assert!(description.contains("a 403 from efr's proxy when it is on"), "{description}");
}
