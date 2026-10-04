use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::{HeaderMap, HeaderName, HeaderValue};
use pretty_assertions::assert_eq;
use rstest::rstest;
use url::Url;

use super::{REDACTED, header_map, header_value, headers, is_sensitive_header, url};

fn name(text: &'static str) -> HeaderName {
    HeaderName::from_static(text)
}

#[rstest]
#[case::authorization("authorization", true)]
#[case::proxy_authorization("proxy-authorization", true)]
#[case::cookie("cookie", true)]
#[case::set_cookie("set-cookie", true)]
#[case::api_key("x-api-key", true)]
#[case::auth_token("x-auth-token", true)]
#[case::session("x-session-id", true)]
#[case::account("chatgpt-account-id", true)]
#[case::organization("openai-organization", true)]
#[case::content_type("content-type", false)]
#[case::accept("accept", false)]
#[case::user_agent("user-agent", false)]
#[case::originator("originator", false)]
#[case::request_id("x-request-id", false)]
#[case::tailscale_cap("tailscale-cap", false)]
#[case::retry_after("retry-after", false)]
fn sensitive_header_names(#[case] header: &'static str, #[case] expected: bool) {
    assert_eq!(is_sensitive_header(&name(header)), expected);
}

#[rstest]
#[case::bearer("Bearer sk-live-123", "Bearer [REDACTED]")]
#[case::basic("Basic dXNlcjpwYXNz", "Basic [REDACTED]")]
#[case::bare_key("sk-live-123", REDACTED)]
#[case::key_with_space("sk-live 123", REDACTED)]
#[case::scheme_only("Bearer ", REDACTED)]
fn authorization_keeps_only_a_plausible_scheme(
    #[case] value: &'static str,
    #[case] expected: &str,
) {
    let redacted = header_value(&AUTHORIZATION, &HeaderValue::from_static(value));
    assert_eq!(redacted.to_str().unwrap(), expected);
}

#[test]
fn other_sensitive_headers_lose_the_whole_value() {
    let redacted = header_value(&COOKIE, &HeaderValue::from_static("session=abc"));
    assert_eq!(redacted.to_str().unwrap(), REDACTED);
}

#[test]
fn a_value_marked_sensitive_is_hidden_under_any_name() {
    let mut value = HeaderValue::from_static("opaque");
    value.set_sensitive(true);
    assert_eq!(header_value(&name("x-custom"), &value).to_str().unwrap(), REDACTED);
}

#[test]
fn harmless_values_pass_unchanged() {
    let value = HeaderValue::from_static("application/json");
    assert_eq!(header_value(&CONTENT_TYPE, &value), value);
}

fn sample() -> HeaderMap {
    let mut map = HeaderMap::new();
    map.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret-token"));
    map.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    map.append(SET_COOKIE, HeaderValue::from_static("a=1"));
    map.append(SET_COOKIE, HeaderValue::from_static("b=2"));
    map
}

#[test]
fn header_map_keeps_names_and_counts() {
    let redacted = header_map(&sample());
    assert_eq!(redacted.len(), 4);
    assert_eq!(redacted[AUTHORIZATION], "Bearer [REDACTED]");
    assert_eq!(redacted[ACCEPT], "text/event-stream");
    let cookies: Vec<_> = redacted.get_all(SET_COOKIE).iter().collect();
    assert_eq!(cookies, [REDACTED, REDACTED]);
}

#[test]
fn display_and_debug_hide_secrets() {
    let map = sample();
    let display = headers(&map).to_string();
    let debug = format!("{:?}", headers(&map));
    for text in [&display, &debug] {
        assert!(!text.contains("secret-token"), "{text}");
        assert!(!text.contains("a=1"), "{text}");
        assert!(text.contains("text/event-stream"), "{text}");
    }
    assert!(display.contains("authorization: Bearer [REDACTED]"), "{display}");
}

#[rstest]
#[case::plain("https://api.openai.com/v1/responses", "https://api.openai.com/v1/responses")]
#[case::harmless_query("https://x.test/a?model=gpt&limit=5", "https://x.test/a?model=gpt&limit=5")]
#[case::oauth_callback(
    "http://localhost:1455/auth/callback?code=abc&state=xyz&scope=openid",
    "http://localhost:1455/auth/callback?code=[REDACTED]&state=[REDACTED]&scope=openid"
)]
#[case::token_params(
    "https://x.test/?access_token=a&refresh_token=b&api_key=c&X-Amz-Signature=d",
    "https://x.test/?access_token=[REDACTED]&refresh_token=[REDACTED]&api_key=[REDACTED]&X-Amz-Signature=[REDACTED]"
)]
#[case::encoded_key(
    "https://x.test/?%74oken=a&q=%20b",
    "https://x.test/?%74oken=[REDACTED]&q=%20b"
)]
#[case::bare_key_kept("https://x.test/?token&flag", "https://x.test/?token&flag")]
#[case::password("https://user:hunter2@x.test/p", "https://user:%5BREDACTED%5D@x.test/p")]
#[case::fragment("https://x.test/cb#access_token=abc", "https://x.test/cb#[REDACTED]")]
#[case::whois(
    "http://local-tailscaled.sock/localapi/v0/whois?addr=100.64.0.1:41641",
    "http://local-tailscaled.sock/localapi/v0/whois?addr=100.64.0.1:41641"
)]
fn url_redaction(#[case] input: &str, #[case] expected: &str) {
    assert_eq!(url(&Url::parse(input).unwrap()), expected);
}
