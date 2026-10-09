use pretty_assertions::assert_eq;

use super::{API_BASE_URL, AnthropicConfig, CacheTtl};
use crate::AnthropicError;

#[test]
fn the_defaults_reach_the_public_api_with_auto_markers() {
    let config = AnthropicConfig::new();
    assert_eq!(config, AnthropicConfig::default());
    assert_eq!(config.base_url(), API_BASE_URL);
    assert_eq!(config.messages_url(), "https://api.anthropic.com/v1/messages");
    assert_eq!(config.models_url(), "https://api.anthropic.com/v1/models");
    assert_eq!(config.cache_ttl(), CacheTtl::Auto);
    assert_eq!(config.workspace_id(), None);
    assert!(config.models().is_empty());
}

#[test]
fn a_base_url_loses_its_trailing_slash() {
    let config = AnthropicConfig::new().with_base_url("http://127.0.0.1:4000/v1/").unwrap();
    assert_eq!(config.messages_url(), "http://127.0.0.1:4000/v1/messages");
    assert_eq!(config.models_url(), "http://127.0.0.1:4000/v1/models");
}

#[test]
fn a_base_url_that_is_not_http_is_refused() {
    let error = AnthropicConfig::new().with_base_url("not a url?key=sk-ant-secret").unwrap_err();
    assert!(matches!(error, AnthropicError::InvalidBaseUrl { .. }), "{error:?}");
}

#[test]
fn a_workspace_id_must_be_a_header_value() {
    let config = AnthropicConfig::new().with_workspace_id("wrkspc_01abc").unwrap();
    assert_eq!(config.workspace_id(), Some("wrkspc_01abc"));
    for refused in ["", "wrkspc\r\nx-injected: 1"] {
        let error = AnthropicConfig::new().with_workspace_id(refused).unwrap_err();
        assert!(matches!(error, AnthropicError::InvalidWorkspaceId { .. }), "{error:?}");
    }
}

#[test]
fn the_cache_ttl_is_kept() {
    let config = AnthropicConfig::new().with_cache_ttl(CacheTtl::OneHour);
    assert_eq!(config.cache_ttl(), CacheTtl::OneHour);
}
