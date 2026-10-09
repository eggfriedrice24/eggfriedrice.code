use pretty_assertions::assert_eq;
use rstest::rstest;

use super::redact;

#[rstest]
#[case::export("export GITHUB_TOKEN=abc123", "export GITHUB_TOKEN=[redacted]")]
#[case::prefix_of_a_command("API_KEY=abc cargo run", "API_KEY=[redacted] cargo run")]
#[case::double_quotes(
    r#"export DB_PASSWORD="a b c" && psql"#,
    "export DB_PASSWORD=[redacted] && psql"
)]
#[case::single_quotes(
    "AWS_SECRET_ACCESS_KEY='x y'; aws s3 ls",
    "AWS_SECRET_ACCESS_KEY=[redacted]; aws s3 ls"
)]
#[case::ansi_c_quotes("MY_TOKEN=$'a\\tb' run", "MY_TOKEN=[redacted] run")]
#[case::lowercase_name("api_key=abc ./run", "api_key=[redacted] ./run")]
#[case::option("tool --api-key=abc --verbose", "tool --api-key=[redacted] --verbose")]
#[case::inside_quotes(
    r#"curl -d "user=me&password=hunter2" x"#,
    r#"curl -d "user=me&password=[redacted]" x"#
)]
#[case::unclosed_quote(r#"TOKEN="abc def"#, "TOKEN=[redacted]")]
#[case::several(
    "A_TOKEN=1 B_SECRET=2 PATH=/bin",
    "A_TOKEN=[redacted] B_SECRET=[redacted] PATH=/bin"
)]
fn the_value_of_a_secret_assignment_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::reference("export GITHUB_TOKEN=$OTHER")]
#[case::substitution("export GITHUB_TOKEN=$(pass show github)")]
#[case::braces("TOKEN=${TOKEN:-none} run")]
#[case::backticks("TOKEN=`pass show x` run")]
#[case::empty("unset TOKEN; TOKEN= run")]
#[case::other_names("PATH=/usr/bin:$PATH RUST_LOG=debug KEYBOARD=us cargo test")]
#[case::comparison("[ \"$a\" == b ]")]
#[case::a_name_alone("echo $GITHUB_TOKEN")]
fn a_line_without_a_secret_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[rstest]
#[case::anthropic(
    "curl -H 'x-api-key: sk-ant-api03-abcdefghijklmnopqrstuv' x",
    "curl -H 'x-api-key: [redacted]' x"
)]
#[case::openai_project(
    "echo sk-proj-ABCDEFGHIJKLMNOPQRST_123 | pbcopy",
    "echo [redacted] | pbcopy"
)]
#[case::openai(
    "Authorization: Bearer sk-0123456789abcdefABCDEF",
    "Authorization: Bearer [redacted]"
)]
#[case::github(
    "gh auth login --with-token <<< ghp_0123456789abcdefABCDEF",
    "gh auth login --with-token <<< [redacted]"
)]
#[case::github_fine_grained("x github_pat_11ABCDEFG0123456789_abcdef y", "x [redacted] y")]
#[case::assignment_and_key(
    "OPENAI_API_KEY=sk-proj-ABCDEFGHIJKLMNOPQRST",
    "OPENAI_API_KEY=[redacted]"
)]
fn a_word_in_the_form_of_a_key_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::short("pip install sk-learn")]
#[case::inside_a_word("ask-0123456789abcdefABCDEF and task-0123456789abcdefABCDEF")]
#[case::not_a_key("git checkout ghp_short")]
fn a_word_that_only_looks_like_a_key_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[test]
fn text_outside_ascii_keeps_its_characters() {
    assert_eq!(redact("écho TÖKEN=ü PASSWORD=ü ✓"), "écho TÖKEN=ü PASSWORD=[redacted] ✓");
}
