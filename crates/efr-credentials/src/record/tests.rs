use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use rstest::rstest;
use secrecy::{ExposeSecret as _, SecretString};

use super::{CredentialRecord, OAuthTokens};
use crate::{CredentialId, CredentialsError};

const EXPIRES_AT: &str = "2026-10-04T12:00:00Z";

fn id() -> CredentialId {
    CredentialId::new("openai-subscription").unwrap()
}

fn full_tokens() -> OAuthTokens {
    let mut tokens = OAuthTokens::new(SecretString::from("access-1"));
    tokens.refresh_token = Some(SecretString::from("refresh-1"));
    tokens.id_token = Some(SecretString::from("header.claims.signature"));
    tokens.expires_at = Some(EXPIRES_AT.parse().unwrap());
    tokens.account_id = Some("acct-42".to_owned());
    tokens
}

fn encode_text(record: &CredentialRecord) -> String {
    String::from_utf8(record.encode(&id()).unwrap().to_vec()).unwrap()
}

fn decode(json: &str) -> Result<CredentialRecord, CredentialsError> {
    CredentialRecord::decode(&id(), json.as_bytes())
}

fn expose(secret: Option<&SecretString>) -> Option<&str> {
    secret.map(|secret| secret.expose_secret())
}

// The on-disk form is a compatibility promise: a file written by this build must stay
// readable by later builds, so the exact text is pinned here.
#[test]
fn api_key_json_form_is_frozen() {
    let record = CredentialRecord::ApiKey { key: SecretString::from("sk-test") };
    assert_eq!(
        encode_text(&record),
        "{\n  \"version\": 1,\n  \"kind\": \"api_key\",\n  \"key\": \"sk-test\"\n}\n"
    );
}

#[test]
fn oauth_json_form_is_frozen() {
    let record = CredentialRecord::OAuth(full_tokens());
    let expected = r#"{
  "version": 1,
  "kind": "oauth",
  "access_token": "access-1",
  "refresh_token": "refresh-1",
  "id_token": "header.claims.signature",
  "expires_at": "2026-10-04T12:00:00Z",
  "account_id": "acct-42"
}
"#;
    assert_eq!(encode_text(&record), expected);
}

#[test]
fn oauth_json_form_omits_absent_fields() {
    let record = CredentialRecord::OAuth(OAuthTokens::new(SecretString::from("only")));
    assert_eq!(
        encode_text(&record),
        "{\n  \"version\": 1,\n  \"kind\": \"oauth\",\n  \"access_token\": \"only\"\n}\n"
    );
}

#[test]
fn api_key_round_trips() {
    let record = CredentialRecord::ApiKey { key: SecretString::from("sk-\"quoted\"\\key") };
    let bytes = record.encode(&id()).unwrap();
    match CredentialRecord::decode(&id(), &bytes).unwrap() {
        CredentialRecord::ApiKey { key } => assert_eq!(key.expose_secret(), "sk-\"quoted\"\\key"),
        other => panic!("unexpected record {other:?}"),
    }
}

#[test]
fn oauth_round_trips() {
    let bytes = CredentialRecord::OAuth(full_tokens()).encode(&id()).unwrap();
    let CredentialRecord::OAuth(tokens) = CredentialRecord::decode(&id(), &bytes).unwrap() else {
        panic!("not an oauth record");
    };
    assert_eq!(tokens.access_token.expose_secret(), "access-1");
    assert_eq!(expose(tokens.refresh_token.as_ref()), Some("refresh-1"));
    assert_eq!(expose(tokens.id_token.as_ref()), Some("header.claims.signature"));
    assert_eq!(tokens.expires_at, Some(EXPIRES_AT.parse().unwrap()));
    assert_eq!(tokens.account_id.as_deref(), Some("acct-42"));
}

#[test]
fn decode_ignores_unknown_fields() {
    let record = decode(r#"{"version":1,"kind":"api_key","key":"k","added_later":true}"#).unwrap();
    assert!(matches!(record, CredentialRecord::ApiKey { .. }));
}

#[test]
fn decode_reports_a_newer_version_before_parsing_the_rest() {
    match decode(r#"{"version":2,"kind":"passkey","key":7}"#) {
        Err(CredentialsError::UnsupportedVersion { id: failed, version: 2 }) => {
            assert_eq!(failed, id());
        }
        other => panic!("unexpected result {other:?}"),
    }
}

#[rstest]
#[case::api_key(r#"{"version":1,"kind":"api_key"}"#, "key")]
#[case::oauth(r#"{"version":1,"kind":"oauth","refresh_token":"r"}"#, "access_token")]
fn decode_reports_a_missing_field(#[case] json: &str, #[case] expected: &str) {
    match decode(json) {
        Err(CredentialsError::MissingField { field, .. }) => assert_eq!(field, expected),
        other => panic!("unexpected result {other:?}"),
    }
}

#[rstest]
#[case::not_json("not json")]
#[case::no_version(r#"{"kind":"api_key","key":"k"}"#)]
#[case::unknown_kind(r#"{"version":1,"kind":"passkey","key":"k"}"#)]
#[case::wrong_type(r#"{"version":1,"kind":"api_key","key":7}"#)]
#[case::bad_timestamp(r#"{"version":1,"kind":"oauth","access_token":"a","expires_at":"soon"}"#)]
fn decode_rejects_malformed_json(#[case] json: &str) {
    assert!(matches!(decode(json), Err(CredentialsError::Decode { .. })), "{json}");
}

#[test]
fn decode_errors_do_not_echo_the_secret() {
    let err = decode(r#"{"version":1,"kind":"api_key","key":["sk-secret"]}"#).unwrap_err();
    let CredentialsError::Decode { source, .. } = &err else { panic!("unexpected error {err:?}") };
    assert!(!err.to_string().contains("sk-secret"));
    assert!(!source.to_string().contains("sk-secret"));
}

#[test]
fn debug_output_redacts_every_secret() {
    let debug = format!("{:?}", CredentialRecord::OAuth(full_tokens()));
    for secret in ["access-1", "refresh-1", "header.claims.signature"] {
        assert!(!debug.contains(secret), "{secret} leaked into {debug}");
    }
    assert!(debug.contains("acct-42"), "the account id is not a secret: {debug}");
}

#[rstest]
#[case::well_before(-60, false)]
#[case::one_second_before(-1, false)]
#[case::exactly_at(0, true)]
#[case::after(1, true)]
fn expires_before_compares_with_the_deadline(#[case] offset_secs: i64, #[case] expected: bool) {
    let expires_at: Timestamp = EXPIRES_AT.parse().unwrap();
    let deadline = expires_at.checked_add(SignedDuration::from_secs(offset_secs)).unwrap();
    assert_eq!(full_tokens().expires_before(deadline), expected);
}

#[test]
fn tokens_without_an_expiry_never_expire() {
    let tokens = OAuthTokens::new(SecretString::from("a"));
    assert!(!tokens.expires_before(Timestamp::MAX));
}
