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

#[rstest]
#[case::https(
    "git clone https://me:hunter2@github.com/o/r",
    "git clone https://me:[redacted]@github.com/o/r"
)]
#[case::token_user(
    "git remote add o https://x-access-token:abc@github.com/o/r.git",
    "git remote add o https://x-access-token:[redacted]@github.com/o/r.git"
)]
#[case::database(
    "psql postgres://app:s3cret@db.local:5432/app",
    "psql postgres://app:[redacted]@db.local:5432/app"
)]
#[case::scheme_with_plus(
    "pip install git+https://u:p@host/r",
    "pip install git+https://u:[redacted]@host/r"
)]
#[case::no_user("curl https://:tok@host/x", "curl https://:[redacted]@host/x")]
#[case::at_in_the_password("curl 'https://u:p@ss@host'", "curl 'https://u:[redacted]@host'")]
#[case::in_quotes(
    r#"export DATABASE_URL="mysql://root:pw@localhost/db""#,
    r#"export DATABASE_URL="mysql://root:[redacted]@localhost/db""#
)]
#[case::outside_ascii("curl https://mé:pä@hôst/", "curl https://mé:[redacted]@hôst/")]
#[case::two("x http://a:1@h1 http://b:2@h2", "x http://a:[redacted]@h1 http://b:[redacted]@h2")]
fn the_password_of_a_url_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::port("curl http://localhost:8080/api")]
#[case::user_only("git clone ssh://git@github.com:22/o/r")]
#[case::scp_form("git clone git@github.com:o/r.git")]
#[case::at_in_the_path("curl https://host:8443/u/me@x.org")]
#[case::at_in_the_query("curl 'https://host:8443/?mail=a@b.org'")]
#[case::ipv6("curl http://[::1]:8080/")]
#[case::reference("git clone https://me:$TOKEN@github.com/o/r")]
#[case::empty_password("curl https://me:@host/")]
#[case::not_a_scheme("echo :// a:b@c")]
fn a_url_without_a_password_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[rstest]
#[case::bearer(
    "curl -H 'Authorization: Bearer abc.def' x",
    "curl -H 'Authorization: Bearer [redacted]' x"
)]
#[case::token(
    r#"curl -H "Authorization: token abc" https://api.github.com"#,
    r#"curl -H "Authorization: token [redacted]" https://api.github.com"#
)]
#[case::basic_without_a_blank(
    "curl -H 'authorization:Basic dXNlcjpwYXNz' x",
    "curl -H 'authorization:Basic [redacted]' x"
)]
#[case::no_scheme("curl -H 'Authorization: abc' x", "curl -H 'Authorization: [redacted]' x")]
#[case::digest(
    r#"curl -H 'Authorization: Digest username="u", response="r"' x"#,
    "curl -H 'Authorization: Digest [redacted]' x"
)]
#[case::proxy(
    "curl -H 'Proxy-Authorization: Basic abc' x",
    "curl -H 'Proxy-Authorization: Basic [redacted]' x"
)]
#[case::api_key_header("curl -H 'X-Api-Key: abc' x", "curl -H 'X-Api-Key: [redacted]' x")]
#[case::gitlab(
    "curl --header 'PRIVATE-TOKEN: glpat-abc' x",
    "curl --header 'PRIVATE-TOKEN: [redacted]' x"
)]
#[case::cookie("curl -H 'Cookie: a=1; session=abc' x", "curl -H 'Cookie: [redacted]' x")]
#[case::attached_option(
    "curl '-HAuthorization: Bearer abc' x",
    "curl '-HAuthorization: Bearer [redacted]' x"
)]
#[case::long_option(
    "wget '--header=Authorization: Bearer abc' x",
    "wget '--header=Authorization: Bearer [redacted]' x"
)]
#[case::httpie(
    "http GET x Authorization:'Bearer abc'",
    "http GET x Authorization:'Bearer [redacted]'"
)]
#[case::git_config(
    r#"git -c http.extraHeader="Authorization: Basic abc" clone x"#,
    r#"git -c http.extraHeader="Authorization: Basic [redacted]" clone x"#
)]
#[case::echo("echo Authorization: Bearer abc && ls", "echo Authorization: Bearer [redacted] && ls")]
#[case::one_word_is_the_secret("echo Authorization: abc", "echo Authorization: [redacted]")]
#[case::here_document_line(
    "cat <<EOF\nAuthorization: Bearer abc\nEOF",
    "cat <<EOF\nAuthorization: Bearer [redacted]\nEOF"
)]
fn the_value_of_a_secret_header_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::reference(r#"curl -H "Authorization: Bearer $TOKEN" x"#)]
#[case::substitution(r#"curl -H "Authorization: Bearer $(gh auth token)" x"#)]
#[case::reference_without_a_scheme(r#"curl -H "X-Api-Key: $KEY" x"#)]
#[case::other_headers("curl -H 'Content-Type: application/json' -H 'Accept: */*' x")]
#[case::grep_pattern(r#"grep -rn "password:" config/"#)]
#[case::grep_pattern_unquoted("grep -rn token: src")]
#[case::empty_value("curl -H 'Authorization:' x")]
#[case::path("cargo run -- token::parse x")]
#[case::url("curl token://host/x")]
#[case::mid_text(r#"git commit -m "fix the token: it leaked""#)]
fn a_header_without_a_secret_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

const FRAGMENTS: &[&str] = &[
    "mysql -p",
    "ü",
    "'",
    "\"",
    "\\",
    "$'",
    "$",
    "`",
    "://",
    "h://ü:é@",
    "@",
    ":",
    "=",
    "TOKEN=",
    "-H",
    "Authorization:",
    " Bearer ",
    "curl -u ",
    "é:",
    "\n",
    "&>",
    ">&",
    "|",
    "sshpass -p",
    "--password=",
    "sk-0123456789abcdefABCDEF",
    "✓",
];

/// Every line of three fragments: the redaction never panics, it cuts no character in
/// two, and a redacted line redacts to itself.
#[test]
fn any_line_redacts_without_a_panic_and_redacts_once() {
    for a in FRAGMENTS {
        for b in FRAGMENTS {
            for c in FRAGMENTS {
                let line = format!("{a}{b}{c}");
                let redacted = redact(&line);
                assert_eq!(redact(&redacted), redacted, "{line:?}");
            }
        }
    }
}
