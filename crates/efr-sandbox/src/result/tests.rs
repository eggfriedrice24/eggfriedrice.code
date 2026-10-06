use efr_protocol::SandboxSummary;
use pretty_assertions::assert_eq;

use crate::result::{SETUP_FAILURE_STATUS, SandboxResult, nonce_hex, parse_nonce_hex};

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
fn a_nonce_is_32_lowercase_hex_digits() {
    let nonce = [0xab_u8; 16];
    let hex = nonce_hex(&nonce);
    assert_eq!(hex, "ab".repeat(16));
    assert_eq!(parse_nonce_hex(&hex), Some(nonce));
    assert_eq!(parse_nonce_hex(&"AB".repeat(16)), None);
    assert_eq!(parse_nonce_hex("abc"), None);
    assert_eq!(parse_nonce_hex(&"g0".repeat(16)), None);
}
