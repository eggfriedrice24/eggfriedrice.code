use std::ffi::OsString;
use std::io;
use std::path::Path;

use efr_protocol::SandboxSummary;
use pretty_assertions::assert_eq;

use crate::layers::{CacheDeletes, CacheLayer, DeleteLimits, LayerEntry, LayerView};
use crate::result::{
    JobStop, MAX_RESULT_BYTES, SETUP_FAILURE_STATUS, SandboxResult, nonce_hex, parse_nonce_hex,
};
use crate::testing::FakeFs;

#[test]
fn the_status_is_the_code_the_signal_or_a_setup_failure() {
    let code = SandboxResult { started: true, exit_code: Some(2), ..SandboxResult::default() };
    assert_eq!(code.status(), 2);
    let signal = SandboxResult { started: true, signal: Some(15), ..SandboxResult::default() };
    assert_eq!(signal.status(), 143);
    let setup = SandboxResult {
        setup_error: Some("Can't mount overlay".to_owned()),
        ..SandboxResult::default()
    };
    assert_eq!(setup.status(), SETUP_FAILURE_STATUS);
}

#[test]
fn a_result_reads_back_and_old_fields_default() {
    let result = SandboxResult {
        started: true,
        exit_code: Some(0),
        summary: SandboxSummary {
            confined: true,
            promoted: vec!["RUST_LOG".to_owned()],
            ..SandboxSummary::default()
        },
        cwd: Some("/tmp/work".into()),
        env_removed: vec!["GITHUB_TOKEN".to_owned()],
        state_kept: true,
        ..SandboxResult::default()
    };
    assert_eq!(SandboxResult::from_json(&result.to_json().unwrap()).unwrap(), result);
    assert_eq!(SandboxResult::from_json(b"{}").unwrap(), SandboxResult::default());
}

#[test]
fn a_job_result_names_its_stop_and_its_nice_value() {
    let result = SandboxResult {
        started: true,
        signal: Some(15),
        stop: Some(JobStop::Deadline),
        nice: Some(19),
        ..SandboxResult::default()
    };
    let text = String::from_utf8(result.to_json().unwrap()).unwrap();
    assert!(text.contains("\"stop\":\"deadline\""), "{text}");
    assert_eq!(SandboxResult::from_json(text.as_bytes()).unwrap(), result);
    // A call's result has neither field.
    let call = String::from_utf8(SandboxResult::default().to_json().unwrap()).unwrap();
    assert!(!call.contains("stop") && !call.contains("nice"), "{call}");
    let lifeline = SandboxResult::from_json(br#"{"stop":"lifeline"}"#).unwrap();
    assert_eq!(lifeline.stop, Some(JobStop::Lifeline));
}

#[test]
fn a_nonce_is_32_lowercase_hex_digits() {
    let nonce = [0xab_u8; 16];
    let hex = nonce_hex(&nonce);
    assert_eq!(hex, "ab".repeat(16));
    assert_eq!(parse_nonce_hex(&hex), Some(nonce));
    assert_eq!(parse_nonce_hex(&"AB".repeat(16)), None);
    assert_eq!(parse_nonce_hex("abc"), None);
    assert_eq!(parse_nonce_hex(&"g0".repeat(16)), None);
}

/// An upper layer whose root holds `names` as whiteouts.
struct Whiteouts(Vec<String>);

impl LayerView for Whiteouts {
    fn entries(&self, _dir: &Path) -> io::Result<Vec<(OsString, LayerEntry)>> {
        Ok(self.0.iter().map(|name| (OsString::from(name), LayerEntry::Whiteout)).collect())
    }
}

#[test]
fn a_full_cache_delete_list_fits_in_the_result() {
    // Long names full of characters that JSON escapes, more than the list holds.
    let names: Vec<String> =
        (0..20_000).map(|n| format!("{n:05}{}", "\"\u{1}".repeat(100))).collect();
    let mut lower = FakeFs::new();
    for name in &names {
        lower.file(&format!("/home/u/.cache/{name}"), "x");
    }
    let layer = CacheLayer {
        target: "/home/u/.cache".into(),
        dir: "/l/0".into(),
        upper: "/l/0/upper".into(),
        work: "/l/0/work".into(),
        moved: Vec::new(),
        fresh: true,
    };
    let mut deletes = CacheDeletes::new(DeleteLimits::default());
    deletes.walk(&layer, &[], &Whiteouts(names), &lower);
    let mut result = SandboxResult::default();
    deletes.fill(&mut result.summary);
    assert!(result.summary.cache_deletes_more > 0, "the list is full");

    let bytes = result.to_json().unwrap();

    assert!(bytes.len() <= SandboxSummary::MAX_CACHE_DELETE_BYTES + 1024, "{}", bytes.len());
    assert!(bytes.len() <= MAX_RESULT_BYTES);
    assert_eq!(SandboxResult::from_json(&bytes).unwrap(), result);
}
