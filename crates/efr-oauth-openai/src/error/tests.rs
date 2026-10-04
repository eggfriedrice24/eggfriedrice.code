use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{GrantKind, OAuthError};
use crate::testing::credential;

fn rejected(grant: GrantKind, status: u16, error: Option<&str>) -> OAuthError {
    OAuthError::TokenRejected {
        grant,
        status,
        error: error.map(str::to_owned),
        description: Some("Invalid refresh token".to_owned()),
    }
}

#[test]
fn messages_name_what_failed_in_one_sentence() {
    let addr: SocketAddr = "127.0.0.1:1455".parse().unwrap();
    let cases = [
        (OAuthError::LoginInProgress, "another login is already in progress"),
        (
            OAuthError::Bind { addr, source: io::Error::from(io::ErrorKind::AddrInUse) },
            "could not listen for the login callback on 127.0.0.1:1455",
        ),
        (
            OAuthError::TimedOut { after: Duration::from_secs(600) },
            "no login callback arrived within 600s",
        ),
        (
            OAuthError::Authorization { error: "access_denied".to_owned(), description: None },
            r#"the authorization server refused the login with "access_denied""#,
        ),
        (OAuthError::MissingCode, "the login callback carried no authorization code"),
        (
            rejected(GrantKind::RefreshToken, 400, Some("invalid_grant")),
            "the token endpoint rejected the refresh token grant with status 400 (invalid_grant)",
        ),
        (
            rejected(GrantKind::AuthorizationCode, 500, None),
            "the token endpoint rejected the authorization code grant with status 500",
        ),
        (
            OAuthError::MalformedJwt { problem: "a part is empty" },
            "a token is not a JWT: a part is empty",
        ),
        (
            OAuthError::NotLoggedIn { id: credential() },
            "no credential is stored as openai-subscription; log in first",
        ),
        (
            OAuthError::NotOAuth { id: credential() },
            "the stored credential openai-subscription is not an OAuth login",
        ),
        (
            OAuthError::NoRefreshToken { id: credential() },
            "the stored credential openai-subscription has no refresh token and its access token can no longer be used",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn only_a_missing_or_refused_login_needs_a_new_login() {
    let needs = [
        OAuthError::NotLoggedIn { id: credential() },
        OAuthError::NotOAuth { id: credential() },
        OAuthError::NoRefreshToken { id: credential() },
        rejected(GrantKind::RefreshToken, 400, Some("invalid_grant")),
        rejected(GrantKind::RefreshToken, 401, None),
    ];
    for error in needs {
        assert!(error.needs_login(), "{error}");
    }
    let does_not = [
        rejected(GrantKind::RefreshToken, 500, None),
        rejected(GrantKind::RefreshToken, 429, None),
        rejected(GrantKind::AuthorizationCode, 400, Some("invalid_grant")),
        OAuthError::LoginInProgress,
        OAuthError::MissingCode,
    ];
    for error in does_not {
        assert!(!error.needs_login(), "{error}");
    }
}

#[test]
fn grant_kinds_read_as_words() {
    assert_eq!(GrantKind::AuthorizationCode.to_string(), "authorization code");
    assert_eq!(GrantKind::RefreshToken.to_string(), "refresh token");
}

#[test]
fn the_error_is_send_sync_and_static() {
    fn assert_bounds<T: std::error::Error + Send + Sync + 'static>() {}
    assert_bounds::<OAuthError>();
}
