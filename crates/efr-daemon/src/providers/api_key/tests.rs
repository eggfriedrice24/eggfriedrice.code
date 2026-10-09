use pretty_assertions::assert_eq;

use super::{hint, validate};
use crate::providers::{ANTHROPIC, API};
use crate::{DaemonError, KeyProblem};

#[test]
fn a_hint_shows_the_known_prefix_and_the_last_four() {
    let cases = [
        ("sk-ant-api03-AbCdEfGhIjKlMnOp-a1b2", "sk-ant-...a1b2"),
        ("sk-proj-AbCdEfGhIjKlMnOpQrSt9f3c", "sk-proj-...9f3c"),
        ("sk-svcacct-AbCdEfGhIjKl7e4d", "sk-svcacct-...7e4d"),
        ("sk-AbCdEfGhIjKlMnOp0042", "sk-...0042"),
        ("AbCdEfGhIjKlMnOp4242", "...4242"),
        // Too short to hide enough of it: no tail.
        ("sk-ant-short1", "sk-ant-..."),
        ("abc", "..."),
    ];
    for (key, shown) in cases {
        assert_eq!(hint(key), shown, "{key}");
    }
}

#[test]
fn a_key_with_a_problem_is_refused_without_its_text() {
    let cases = [
        (API, "", KeyProblem::Empty),
        (API, "sk-proj-abc def", KeyProblem::Whitespace),
        (ANTHROPIC, "sk-ant-abc\n", KeyProblem::Whitespace),
        (ANTHROPIC, "sk-ant-ab\u{e9}c", KeyProblem::NotAscii),
        (API, "sk-admin-AbCdEfGh", KeyProblem::AdminKey),
    ];
    for (provider, key, problem) in cases {
        let error = validate(provider, key).unwrap_err();

        let DaemonError::InvalidApiKey { problem: found, .. } = &error else { panic!("{error:?}") };
        assert_eq!(*found, problem, "{key:?}");
        if !key.is_empty() {
            for shown in [error.to_string(), format!("{error:?}")] {
                assert!(!shown.contains(key.trim()), "{shown}");
            }
        }
    }
}

#[test]
fn an_anthropic_key_may_start_like_an_admin_key_of_openai() {
    validate(ANTHROPIC, "sk-admin-AbCdEfGh").unwrap();
    validate(API, "sk-proj-AbCdEfGh").unwrap();
}
