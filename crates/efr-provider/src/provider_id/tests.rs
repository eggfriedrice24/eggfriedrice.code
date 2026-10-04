use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

use super::ProviderId;
use crate::ProviderError;

#[rstest]
#[case::subscription("openai-subscription")]
#[case::api_key("openai-api")]
#[case::vendor("anthropic")]
#[case::digit_first("1provider")]
#[case::single("a")]
#[case::trailing_hyphen("replay-")]
fn accepts(#[case] id: &str) {
    assert_eq!(ProviderId::new(id).unwrap().as_str(), id);
}

#[rstest]
#[case::empty("")]
#[case::uppercase("OpenAI")]
#[case::space("open ai")]
#[case::underscore("openai_api")]
#[case::dot("openai.api")]
#[case::slash("openai/api")]
#[case::leading_hyphen("-openai")]
#[case::non_ascii("open\u{e9}")]
fn rejects(#[case] id: &str) {
    match ProviderId::new(id) {
        Err(ProviderError::InvalidProviderId { id: rejected }) => assert_eq!(rejected, id),
        other => panic!("{id:?} gave {other:?}"),
    }
}

#[test]
fn length_limit_is_inclusive() {
    let longest = "a".repeat(ProviderId::MAX_LEN);
    assert!(ProviderId::new(longest.clone()).is_ok());
    assert!(ProviderId::new(longest + "a").is_err());
}

#[test]
fn parses_displays_and_serializes_as_plain_text() {
    let id: ProviderId = "openai-api".parse().unwrap();
    assert_eq!(id.to_string(), "openai-api");
    assert_eq!(id.as_ref(), "openai-api");
    assert_eq!(serde_json::to_value(&id).unwrap(), json!("openai-api"));
    assert_eq!(serde_json::from_value::<ProviderId>(json!("openai-api")).unwrap(), id);
    assert_eq!(String::from(id), "openai-api");
}

#[test]
fn decoding_checks_the_rules() {
    assert!(serde_json::from_value::<ProviderId>(json!("Open AI")).is_err());
}
