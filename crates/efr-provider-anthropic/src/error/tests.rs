use std::error::Error as _;

use pretty_assertions::assert_eq;

use super::AnthropicError;
use crate::AnthropicConfig;

#[test]
fn an_invalid_base_url_keeps_the_cause_but_not_the_url() {
    let error = AnthropicConfig::new().with_base_url("not a url?key=sk-ant-secret").unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("sk-ant-secret"), "{shown}");
    assert!(error.source().is_some());
}

#[test]
fn an_invalid_workspace_id_names_the_value() {
    let error = AnthropicError::InvalidWorkspaceId { workspace_id: "a\nb".to_owned() };
    assert_eq!(error.to_string(), "the workspace id \"a\\nb\" is not a valid header value");
}
