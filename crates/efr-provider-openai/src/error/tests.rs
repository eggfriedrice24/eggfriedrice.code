use std::error::Error as _;

use pretty_assertions::assert_eq;

use super::OpenAiError;
use crate::OpenAiConfig;

#[test]
fn an_invalid_base_url_keeps_the_cause_but_not_the_url() {
    let error = OpenAiConfig::api().with_base_url("not a url?key=sk-secret").unwrap_err();
    assert!(matches!(error, OpenAiError::InvalidBaseUrl { .. }), "{error:?}");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("sk-secret"), "{shown}");
    assert!(error.source().is_some());
}

#[test]
fn an_invalid_originator_names_the_value() {
    let error = OpenAiConfig::subscription().with_originator("efr\r\nx-injected: 1").unwrap_err();
    assert_eq!(
        error.to_string(),
        "the originator \"efr\\r\\nx-injected: 1\" is not a valid header value"
    );
}
