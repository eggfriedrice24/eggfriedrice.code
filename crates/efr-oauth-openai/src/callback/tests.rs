use std::io;

use efr_http::{HttpClient, HttpRequest, header};
use efr_test_support::TestClock;
use hyper::{Method, StatusCode, Uri};
use pretty_assertions::assert_eq;
use secrecy::{ExposeSecret as _, SecretString};
use tokio::net::TcpStream;

use super::{
    CallbackListener, FAILED_PAGE, LoginSlot, Outcome, Reply, STALE_PAGE, SUCCESS_PAGE,
    constant_time_eq, reply,
};
use crate::OAuthError;
use crate::testing::http;

const STATE: &str = "expected-state";

fn get(uri: &str) -> Reply {
    reply(&Method::GET, &uri.parse::<Uri>().unwrap(), STATE)
}

fn code_of(outcome: Option<Outcome>) -> String {
    outcome.unwrap().unwrap().expose_secret().to_owned()
}

#[test]
fn a_callback_with_the_state_and_a_code_succeeds() {
    let Reply { status, page, outcome } = get("/auth/callback?code=abc&state=expected-state");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page, SUCCESS_PAGE);
    assert_eq!(code_of(outcome), "abc");
}

#[test]
fn parameters_are_percent_decoded() {
    let Reply { outcome, .. } = get("/auth/callback?state=expected-state&code=a%2Fb%2Bc");
    assert_eq!(code_of(outcome), "a/b+c");
}

#[test]
fn a_wrong_or_missing_state_is_answered_and_decides_nothing() {
    for uri in [
        "/auth/callback?code=abc&state=other-state",
        "/auth/callback?code=abc&state=expected-stat",
        "/auth/callback?code=abc",
        "/auth/callback",
        "/auth/callback?code=abc&state=other-state&state=expected-state",
    ] {
        let Reply { status, page, outcome } = get(uri);
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(page, STALE_PAGE, "{uri}");
        assert!(outcome.is_none(), "{uri}");
    }
}

#[test]
fn an_error_with_the_state_ends_the_login() {
    let Reply { status, page, outcome } = get(
        "/auth/callback?state=expected-state&error=access_denied&error_description=User+declined",
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(page, FAILED_PAGE);
    let error = outcome.unwrap().unwrap_err();
    assert!(
        matches!(
            error,
            OAuthError::Authorization { ref error, ref description }
                if error == "access_denied" && description.as_deref() == Some("User declined")
        ),
        "{error:?}"
    );
}

#[test]
fn an_error_without_the_state_decides_nothing() {
    let Reply { page, outcome, .. } = get("/auth/callback?error=access_denied&state=forged");
    assert_eq!(page, STALE_PAGE);
    assert!(outcome.is_none());
}

#[test]
fn a_missing_or_empty_code_with_the_state_ends_the_login() {
    for uri in ["/auth/callback?state=expected-state", "/auth/callback?state=expected-state&code="]
    {
        let Reply { page, outcome, .. } = get(uri);
        assert_eq!(page, FAILED_PAGE, "{uri}");
        assert!(matches!(outcome, Some(Err(OAuthError::MissingCode))), "{uri}");
    }
}

#[test]
fn other_paths_and_methods_are_refused() {
    let Reply { status, outcome, .. } = get("/favicon.ico");
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(outcome.is_none());
    let uri: Uri = "/auth/callback?code=abc&state=expected-state".parse().unwrap();
    let Reply { status, outcome, .. } = reply(&Method::POST, &uri, STATE);
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(outcome.is_none());
}

#[test]
fn pages_are_complete_html() {
    for page in [SUCCESS_PAGE, STALE_PAGE, FAILED_PAGE] {
        assert!(page.starts_with("<!doctype html>"), "{page}");
        assert!(page.ends_with("</html>\n"), "{page}");
    }
    assert!(SUCCESS_PAGE.contains("return to the terminal"));
}

#[test]
fn equal_strings_compare_equal_and_others_do_not() {
    assert!(constant_time_eq("abc", "abc"));
    assert!(!constant_time_eq("abc", "abz"));
    assert!(!constant_time_eq("abc", "ab"));
    assert!(constant_time_eq("", ""));
}

#[test]
fn one_login_holds_the_slot_at_a_time() {
    let slot = LoginSlot::default();
    let first = slot.claim().unwrap();
    assert!(matches!(slot.clone().claim(), Err(OAuthError::LoginInProgress)));
    drop(first);
    assert!(slot.claim().is_ok());
}

/// Sends a GET for `path_and_query` to the listener on `port`.
async fn browse(client: &HttpClient, port: u16, path_and_query: &str) -> (u16, String, String) {
    let url = format!("http://127.0.0.1:{port}{path_and_query}");
    let response = client.send(&HttpRequest::get(&url).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers();
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let content_type = headers[header::CONTENT_TYPE].to_str().unwrap().to_owned();
    (status, content_type, response.text().await.unwrap())
}

#[tokio::test]
async fn the_listener_serves_until_the_right_callback_and_returns_its_code() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(SecretString::from(STATE)));
    let client = http(&TestClock::new());

    let (status, _, body) = browse(&client, port, "/favicon.ico").await;
    assert_eq!((status, body.as_str()), (404, "not found\n"));
    let (status, _, body) = browse(&client, port, "/auth/callback?code=x&state=stale").await;
    assert_eq!((status, body.as_str()), (400, STALE_PAGE));
    assert!(!waiting.is_finished());

    let (status, content_type, body) =
        browse(&client, port, "/auth/callback?code=the-code&state=expected-state").await;
    assert_eq!((status, body.as_str()), (200, SUCCESS_PAGE));
    assert_eq!(content_type, "text/html; charset=utf-8");
    let code = waiting.await.unwrap().unwrap();
    assert_eq!(code.expose_secret(), "the-code");
}

#[tokio::test]
async fn an_idle_connection_does_not_hold_up_the_callback() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(SecretString::from(STATE)));
    // A browser may open a connection it never uses.
    let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

    let client = http(&TestClock::new());
    let (status, _, _) =
        browse(&client, port, "/auth/callback?code=the-code&state=expected-state").await;
    assert_eq!(status, 200);
    assert_eq!(waiting.await.unwrap().unwrap().expose_secret(), "the-code");
}

#[tokio::test]
async fn an_authorization_error_ends_the_wait_after_its_page() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(SecretString::from(STATE)));
    let client = http(&TestClock::new());

    let (status, _, body) =
        browse(&client, port, "/auth/callback?error=access_denied&state=expected-state").await;
    assert_eq!((status, body.as_str()), (400, FAILED_PAGE));
    let error = waiting.await.unwrap().unwrap_err();
    assert!(matches!(error, OAuthError::Authorization { .. }), "{error:?}");
}

#[tokio::test]
async fn the_port_is_free_again_once_the_wait_ends() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let port = listener.port();
    let waiting = tokio::spawn(listener.wait(SecretString::from(STATE)));
    let client = http(&TestClock::new());
    browse(&client, port, "/auth/callback?code=c&state=expected-state").await;
    waiting.await.unwrap().unwrap();

    let error = TcpStream::connect(("127.0.0.1", port)).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    CallbackListener::bind(port).await.unwrap();
}

#[tokio::test]
async fn a_port_held_by_another_listener_cannot_be_bound() {
    let held = CallbackListener::bind(0).await.unwrap();
    let error = CallbackListener::bind(held.port()).await.unwrap_err();
    let OAuthError::Bind { addr, source } = error else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(addr.port(), held.port());
    assert!(addr.ip().is_loopback());
    assert_eq!(source.kind(), io::ErrorKind::AddrInUse);
}
