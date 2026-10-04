use pretty_assertions::assert_eq;
use rstest::rstest;

use super::CredentialId;
use crate::CredentialsError;

#[rstest]
#[case::subscription("openai-subscription")]
#[case::api_key("openai-api")]
#[case::dotted("mcp.github")]
#[case::underscore("mcp_server_2")]
#[case::digit_first("1password")]
#[case::single("a")]
fn accepts(#[case] id: &str) {
    assert_eq!(CredentialId::new(id).unwrap().as_str(), id);
}

#[rstest]
#[case::empty("")]
#[case::hidden(".openai")]
#[case::dot(".")]
#[case::dot_dot("..")]
#[case::separator("openai/api")]
#[case::traversal("../openai")]
#[case::uppercase("OpenAI")]
#[case::space("open ai")]
#[case::leading_hyphen("-openai")]
#[case::non_ascii("open\u{e9}")]
#[case::nul("openai\0")]
fn rejects(#[case] id: &str) {
    match CredentialId::new(id) {
        Err(CredentialsError::InvalidId { id: rejected }) => assert_eq!(rejected, id),
        other => panic!("{id:?} gave {other:?}"),
    }
}

#[test]
fn length_limit_is_inclusive() {
    let longest = "a".repeat(CredentialId::MAX_LEN);
    assert!(CredentialId::new(longest.clone()).is_ok());
    assert!(CredentialId::new(longest + "a").is_err());
}

#[test]
fn parses_and_displays_the_same_text() {
    let id: CredentialId = "openai-api".parse().unwrap();
    assert_eq!(id.to_string(), "openai-api");
    assert_eq!(id.as_ref(), "openai-api");
}
