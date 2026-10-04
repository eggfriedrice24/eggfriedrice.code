use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use pretty_assertions::assert_eq;
use secrecy::SecretString;
use serde_json::json;

use super::{Claims, of_tokens, parse};
use crate::OAuthError;
use crate::testing::{ACCOUNT, EMAIL, at, jwt};

#[test]
fn reads_the_account_email_and_expiry_where_openai_puts_them() {
    let token = jwt(&json!({
        "email": EMAIL,
        "exp": at(3600).as_second(),
        "https://api.openai.com/auth": {
            "chatgpt_account_id": ACCOUNT,
            "chatgpt_plan_type": "plus",
        },
    }));
    let claims = parse(&token).unwrap();
    assert_eq!(
        claims,
        Claims {
            account_id: Some(ACCOUNT.to_owned()),
            email: Some(EMAIL.to_owned()),
            expires_at: Some(at(3600)),
        }
    );
}

#[test]
fn a_top_level_account_id_is_the_fallback() {
    let claims = parse(&jwt(&json!({ "chatgpt_account_id": "top-level" }))).unwrap();
    assert_eq!(claims.account_id.as_deref(), Some("top-level"));
}

#[test]
fn the_namespaced_account_id_wins_over_the_top_level_one() {
    let claims = parse(&jwt(&json!({
        "chatgpt_account_id": "top-level",
        "https://api.openai.com/auth": { "chatgpt_account_id": "namespaced" },
    })))
    .unwrap();
    assert_eq!(claims.account_id.as_deref(), Some("namespaced"));
}

#[test]
fn the_first_organization_is_not_taken_for_an_account() {
    let claims = parse(&jwt(&json!({ "organizations": [{ "id": "org-1" }] }))).unwrap();
    assert_eq!(claims.account_id, None);
}

#[test]
fn the_profile_email_is_the_fallback() {
    let claims =
        parse(&jwt(&json!({ "https://api.openai.com/profile": { "email": EMAIL } }))).unwrap();
    assert_eq!(claims.email.as_deref(), Some(EMAIL));
}

#[test]
fn a_fractional_expiry_rounds_down() {
    let claims = parse(&jwt(&json!({ "exp": 1_791_118_800.75 }))).unwrap();
    assert_eq!(claims.expires_at, Some(at(3600)));
}

#[test]
fn claims_of_an_unexpected_shape_read_as_missing() {
    let claims = parse(&jwt(&json!({
        "exp": "tomorrow",
        "email": 42,
        "chatgpt_account_id": "",
        "https://api.openai.com/auth": "not an object",
    })))
    .unwrap();
    assert_eq!(claims, Claims::default());
}

#[test]
fn an_expiry_out_of_range_reads_as_missing() {
    let claims = parse(&jwt(&json!({ "exp": 1e300 }))).unwrap();
    assert_eq!(claims.expires_at, None);
}

#[test]
fn a_padded_payload_is_accepted() {
    let payload = URL_SAFE.encode(br#"{"chatgpt_account_id":"padded"}"#);
    assert!(payload.ends_with('='), "{payload}");
    let claims = parse(&format!("e30.{payload}.sig")).unwrap();
    assert_eq!(claims.account_id.as_deref(), Some("padded"));
}

#[test]
fn a_token_without_three_parts_is_malformed() {
    for token in ["opaque", "a.b", "a.b.c.d"] {
        let error = parse(token).unwrap_err();
        assert!(matches!(error, OAuthError::MalformedJwt { .. }), "{token}: {error}");
    }
}

#[test]
fn an_empty_part_is_malformed() {
    for token in [".e30.sig", "e30..sig"] {
        assert!(matches!(parse(token), Err(OAuthError::MalformedJwt { .. })), "{token}");
    }
}

#[test]
fn a_payload_that_is_not_base64url_is_malformed() {
    assert!(matches!(parse("e30.not*base64.sig"), Err(OAuthError::MalformedJwt { .. })));
}

#[test]
fn a_payload_that_is_not_a_json_object_is_an_error() {
    for payload in [&b"not json"[..], b"[1,2]"] {
        let token = format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(payload));
        assert!(matches!(parse(&token), Err(OAuthError::JwtClaims { .. })), "{token}");
    }
}

#[test]
fn the_id_token_names_the_account_and_the_access_token_the_expiry() {
    let id = SecretString::from(jwt(&json!({
        "email": EMAIL,
        "exp": at(60).as_second(),
        "https://api.openai.com/auth": { "chatgpt_account_id": "from-id" },
    })));
    let access = SecretString::from(jwt(&json!({
        "exp": at(3600).as_second(),
        "https://api.openai.com/auth": { "chatgpt_account_id": "from-access" },
    })));
    let claims = of_tokens(Some(&id), &access);
    assert_eq!(claims.account_id.as_deref(), Some("from-id"));
    assert_eq!(claims.email.as_deref(), Some(EMAIL));
    assert_eq!(claims.expires_at, Some(at(3600)));
}

#[test]
fn the_access_token_names_the_account_when_the_id_token_cannot() {
    let id = SecretString::from("opaque-id-token");
    let access = SecretString::from(jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "from-access" },
    })));
    assert_eq!(of_tokens(Some(&id), &access).account_id.as_deref(), Some("from-access"));
    assert_eq!(of_tokens(None, &access).account_id.as_deref(), Some("from-access"));
}

#[test]
fn opaque_tokens_have_no_claims() {
    let claims = of_tokens(None, &SecretString::from("sk-opaque"));
    assert_eq!(claims, Claims::default());
}
