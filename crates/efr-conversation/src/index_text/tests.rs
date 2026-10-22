use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{REDACTION_VERSION, redact_for_index};

/// Each form that the index redaction recognises, with its result: the forms of the
/// preamble's line redaction, now marked `[***]`, and the forms of the pass over the
/// whole text.
#[rstest]
#[case::assignment("export GITHUB_TOKEN=abc123", "export GITHUB_TOKEN=[***]")]
#[case::key_prefix(
    "my key is sk-ant-api03-abcdefghijklmnopqrstuv, keep it",
    "my key is [***], keep it"
)]
#[case::github_key("echo ghp_0123456789abcdefABCDEF0123456789abcd", "echo [***]")]
#[case::aws_key("aws_access_key_id AKIAIOSFODNN7EXAMPLE", "aws_access_key_id [***]")]
#[case::random_word("the key u8jzPde0IgxLd6GncfBAepfJBd0Kq7Zr2LmN works", "the key [***] works")]
#[case::web_token(
    "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
    "token [***]"
)]
#[case::url_password(
    "clone https://me:hunter2@github.com/o/r",
    "clone https://me:[***]@github.com/o/r"
)]
#[case::header(
    "curl -H 'Authorization: Bearer abc123' https://api.example",
    "curl -H 'Authorization: Bearer [***]' https://api.example"
)]
#[case::option("mysql -u root -phunter2 app", "mysql -u root -p[***] app")]
#[case::yaml("db:\n  password: hunter2xyz\n  user: app", "db:\n  password: [***]\n  user: app")]
#[case::yaml_quoted("  api_key: 'abc 123'", "  api_key: '[***]'")]
#[case::yaml_list_item("- client_secret: abc123", "- client_secret: [***]")]
#[case::toml("api_key = \"abc123\"", "api_key = \"[***]\"")]
#[case::toml_dotted("auth.password = hunter2", "auth.password = [***]")]
#[case::ini_blank_after("password= hunter2", "password= [***]")]
#[case::json(
    "{\"user\": \"me\", \"password\": \"hunter2\"}",
    "{\"user\": \"me\", \"password\": \"[***]\"}"
)]
#[case::json_escaped_quote(r#"{"token": "a\"b", "x": 1}"#, r#"{"token": "[***]", "x": 1}"#)]
#[case::json_bare_value(
    "{\"session_id\": 123456, \"ok\": true}",
    "{\"session_id\": [***], \"ok\": true}"
)]
#[case::json_two_lines(
    "{\n  \"aws_secret_access_key\": \"abc\"\n}",
    "{\n  \"aws_secret_access_key\": \"[***]\"\n}"
)]
#[case::pem(
    "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nabcdef\n-----END RSA PRIVATE KEY-----\ndone",
    "key:\n[***]\ndone"
)]
#[case::pem_cut("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nabc", "[***]")]
#[case::two_pems(
    "-----BEGIN EC PRIVATE KEY-----\na\n-----END EC PRIVATE KEY-----\nand\n-----BEGIN PRIVATE KEY-----\nb\n-----END PRIVATE KEY-----",
    "[***]\nand\n[***]"
)]
fn index_redaction_corpus(#[case] text: &str, #[case] redacted: &str) {
    assert_eq!(redact_for_index(text), redacted);
}

/// Text with no secret, also text that only looks like the forms, stays as it is.
#[rstest]
#[case::prose("Use `cargo nextest run` to run the tests, then open a PR.")]
#[case::command("rsync -av ./site/ laptop:/srv/site/")]
#[case::yaml_other("  user: app\n  port: 5432")]
#[case::reference("password: ${{ secrets.DB_PASSWORD }}")]
#[case::env_reference("api_key = $API_KEY")]
#[case::yaml_block("private_key: |")]
#[case::empty_value("password:")]
#[case::count("max_tokens = 4096")]
#[case::json_other("{\"name\": \"efr\", \"tokens\": 5}")]
#[case::public_key("-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq\n-----END PUBLIC KEY-----")]
#[case::certificate(
    "-----BEGIN CERTIFICATE-----\nMIIDdzCCAl+gAwIBAgIEAgAAuTANBgkq\n-----END CERTIFICATE-----"
)]
#[case::grep_pattern(r#"grep -rn "password:" config/"#)]
#[case::path("cat /home/u/p/App2/src/Components/UserProfileCard2/Index.tsx")]
#[case::git_id("git show 3f786850e387550fdab836ed7e6dc881de23001b")]
#[case::line_starts_outside_ascii("écrire le fichier\n題名 zebra\n🚀 deploy")]
#[case::name_then_a_sign_outside_ascii("password→hunter2")]
fn text_without_a_secret_stays(#[case] text: &str) {
    assert_eq!(redact_for_index(text), text);
}

/// The forms that the index redaction does not recognise. They are known misses, as
/// the changelog says: efr cannot recognise every secret.
#[rstest]
#[case::prose("my password is hunter2, keep it safe")]
#[case::lower_case_hex(
    "the key is 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
)]
#[case::yaml_value_on_the_next_line("password:\n  hunter2xyz")]
#[case::webhook_with_a_lower_case_path(
    "https://hooks.example/services/t00000000/b00000000/xxxxxxxxxxxx"
)]
#[case::name_value_inside_prose("the config has password: hunter2 in it")]
fn a_known_miss_stays(#[case] text: &str) {
    assert_eq!(redact_for_index(text), text);
}

/// The marker has no letter or digit, so the tokenizer of the index makes no word of
/// it.
#[test]
fn the_marker_makes_no_word() {
    let redacted = redact_for_index("export GITHUB_TOKEN=abc123");
    let marker = &redacted["export GITHUB_TOKEN=".len()..];
    assert_eq!(marker, "[***]");
    assert!(!marker.chars().any(char::is_alphanumeric));
}

/// A redacted text redacts to itself, so a second pass changes nothing.
#[rstest]
#[case("db:\n  password: hunter2xyz")]
#[case("{\"token\": \"x\"}")]
#[case("-----BEGIN RSA PRIVATE KEY-----\nabc")]
#[case("export GITHUB_TOKEN=abc123 && echo sk-ant-api03-abcdefghijklmnopqrstuv")]
fn a_redacted_text_redacts_to_itself(#[case] text: &str) {
    let once = redact_for_index(text);
    assert_eq!(redact_for_index(&once), once);
}

/// The lines keep their places: no line is added or removed, and a line end at the end
/// of the text stays.
#[test]
fn the_lines_keep_their_places() {
    let text = "a\n\npassword: x\n";
    assert_eq!(redact_for_index(text), "a\n\npassword: [***]\n");
}

#[test]
fn the_version_starts_at_one() {
    assert_eq!(REDACTION_VERSION, 1);
}
