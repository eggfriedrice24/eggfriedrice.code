use std::time::Duration;

use efr_protocol::ErrorCode;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{bounded_tail, provider_failure};

#[test]
fn provider_errors_map_to_codes_a_client_can_act_on() {
    use efr_provider::ProviderError;
    assert_eq!(provider_failure(&ProviderError::NotLoggedIn).code, ErrorCode::Unauthorized);
    assert_eq!(
        provider_failure(&ProviderError::UnknownModel { model: "m".to_owned() }).code,
        ErrorCode::Invalid
    );
    assert_eq!(provider_failure(&ProviderError::Incomplete).code, ErrorCode::Internal);
    let limited =
        provider_failure(&ProviderError::RateLimited { retry_after: Some(Duration::from_secs(2)) });
    assert_eq!(limited.code, ErrorCode::Busy);
    assert_eq!(limited.data, Some(json!({ "retry_after_ms": 2000 })));
}

#[test]
fn an_output_tail_is_cut_on_a_character_boundary() {
    let long = format!("{}{}", "é".repeat(3000), "end");
    let tail = bounded_tail(&long);
    assert!(tail.len() <= super::TAIL_MAX);
    assert!(tail.ends_with("end"));
    assert_eq!(bounded_tail("short"), "short");
}
