use super::{is_variable_name, matches, secret_like};

#[test]
fn a_star_matches_any_text_and_case_matters() {
    assert!(matches("LC_*", "LC_ALL"));
    assert!(matches("LC_*", "LC_"));
    assert!(!matches("LC_*", "LANG"));
    assert!(matches("*_proxy", "https_proxy"));
    assert!(!matches("*_proxy", "HTTPS_PROXY"));
    assert!(matches("*TOKEN*", "GH_TOKEN_X"));
    assert!(matches("*a*b*", "xaybz"));
    assert!(!matches("*xy*yx", "xyx"));
    assert!(matches("PATH", "PATH"));
    assert!(!matches("PATH", "PATHX"));
    assert!(matches("*", ""));
}

#[test]
fn secret_like_names_are_found_without_case() {
    for name in ["GITHUB_TOKEN", "api_key", "OPENAI_API_KEY", "AWS_REGION", "my_pat", "DB_PASSWORD"]
    {
        assert!(secret_like(name), "{name}");
    }
    for name in ["PATH", "HOME", "RUST_LOG", "KEYBOARD", "PATTERN"] {
        assert!(!secret_like(name), "{name}");
    }
}

#[test]
fn variable_names_are_shell_names() {
    assert!(is_variable_name("_A1"));
    assert!(!is_variable_name("1A"));
    assert!(!is_variable_name("A-B"));
    assert!(!is_variable_name(""));
    assert!(!is_variable_name(&"A".repeat(129)));
}
