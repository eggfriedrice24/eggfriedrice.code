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
#[case::tokens_with_a_secret_word(
    "export GITHUB_TOKENS_SECRET=abc",
    "export GITHUB_TOKENS_SECRET=[redacted]"
)]
#[case::option("tool --api-key=abc --verbose", "tool --api-key=[redacted] --verbose")]
#[case::inside_quotes(
    r#"curl -d "user=me&password=hunter2" x"#,
    r#"curl -d "user=me&password=[redacted]" x"#
)]
#[case::unclosed_quote(r#"TOKEN="abc def"#, "TOKEN=[redacted]")]
#[case::escaped_blank(r"TOKEN=abc\ def cmd", "TOKEN=[redacted] cmd")]
#[case::backslash_at_the_end(r"TOKEN=abc\", "TOKEN=[redacted]")]
#[case::several(
    "A_TOKEN=1 B_SECRET=2 PATH=/bin",
    "A_TOKEN=[redacted] B_SECRET=[redacted] PATH=/bin"
)]
#[case::in_an_option_value(
    "kubectl create secret generic s --from-literal=password=hunter2",
    "kubectl create secret generic s --from-literal=password=[redacted]"
)]
#[case::docker_env(
    "docker run --env=API_TOKEN=abc123 img",
    "docker run --env=API_TOKEN=[redacted] img"
)]
#[case::docker_build_arg(
    "docker build --build-arg=NPM_TOKEN=abc123 .",
    "docker build --build-arg=NPM_TOKEN=[redacted] ."
)]
#[case::helm_set(
    "helm install x --set=auth.password=abc ./chart",
    "helm install x --set=auth.password=[redacted] ./chart"
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
#[case::max_tokens("llm --max-tokens=4096")]
#[case::tokenizer("python train.py --tokenizer=gpt2 --max_tokens=512")]
#[case::token_count("make TOKEN_COUNT=5")]
#[case::tokenizers_parallelism("TOKENIZERS_PARALLELISM=false python x.py")]
#[case::password_length("pwgen PASSWORD_LENGTH=20")]
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

#[rstest]
#[case::aws(
    "aws configure set aws_access_key_id AKIAOC9IS0J8HT9LGMXG",
    "aws configure set aws_access_key_id [redacted]"
)]
#[case::slack_bot("echo xoxb-1234-5678-Db_xHKas1VOqg6YYZYn9 | x", "echo [redacted] | x")]
#[case::slack_user("echo xoxp-1234-5678-Db_xHKas1VOqg6YYZYn9", "echo [redacted]")]
#[case::gitlab(
    "glab auth login --token glpat-Db_xHKas1VOqg6YYZYn9",
    "glab auth login --token [redacted]"
)]
#[case::google("echo AIzaihA-2O76UMFxFkM-R5Kjp1vRt_1fjORS-6i", "echo [redacted]")]
#[case::npm(
    "npm config set //r.test/:_authToken npm_1f8rESQedUStPKR0CsTy4Qwb8DwkNhFdnXsi",
    "npm config set //r.test/:_authToken [redacted]"
)]
#[case::hugging_face(
    "huggingface-cli login --token hf_Vpzz63FfkCzJr4i0B3JrTAwR4y9ojfljoQ",
    "huggingface-cli login --token [redacted]"
)]
#[case::pypi(
    "twine upload -p pypi-AgEIcHlwaS5vcmcCJDk3 dist/*",
    "twine upload -p [redacted] dist/*"
)]
#[case::github(
    "git push https://ghp_1f8rESQedUStPKR0CsTy4Qwb8DwkNhF@x.test/r",
    "git push https://[redacted]@x.test/r"
)]
fn a_key_of_a_known_service_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::npm_setting("echo $npm_config_userconfig npm_config_registry_of_the_project")]
#[case::hugging_face_name(
    "python -c 'from huggingface_hub import hf_hub_download_with_a_long_name'"
)]
#[case::short_aws("echo AKIA1234")]
#[case::lowercase_aws("echo akiaoc9is0j8ht9lgmxg")]
fn a_name_that_starts_like_a_key_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[rstest]
#[case::random_word(
    "./deploy --secret u8jzPde0IgxLd6GncfBAepfJBd0Kh8oOOL8dKLzd",
    "./deploy --secret [redacted]"
)]
#[case::base64_with_padding(
    "echo dThqelBkZTBJZ3hMZDZHbmNmQkFlcGZKQmQwS2g4b09PTDhkSw== | base64 -d",
    "echo [redacted] | base64 -d"
)]
#[case::base64_with_a_slash(
    "echo wJalrXUtnFEMI/K7MDENG/bPxRfiCY0EXAMPLEKEY > k",
    "echo [redacted] > k"
)]
#[case::aws_secret_with_a_lower_case_end(
    "echo wJalrXUtnFEMIK7MDENGbPxRfiCY0EX/abc > k",
    "echo [redacted] > k"
)]
#[case::aws_secret_with_a_lower_case_start(
    "echo xyz/wJalrXUtnFEMIK7MDENGbPxRfiCY0EXAMPLEKEY > k",
    "echo [redacted] > k"
)]
#[case::aws_secret_with_a_slash_first(
    "echo /wJalrXUtnFEMIK7MDENGbPxRfiCY0EXAMPLEKEY > k",
    "echo /[redacted] > k"
)]
#[case::jwt(
    "curl -H 'X: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.u8jzPde0IgxLd6GncfBAepfJBd0' x",
    "curl -H 'X: [redacted]' x"
)]
#[case::jwt_without_a_signature("echo eyJhbGciOiJub25lIn0.eyJzdWIiOiIxIn0.", "echo [redacted]")]
fn a_word_that_looks_random_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::git_id("git show 3f786850e387550fdab836ed7e6dc881de23001b")]
#[case::git_id_upper_case("git show 3F786850E387550FDAB836ED7E6DC881DE23001B")]
#[case::sha256(
    "echo e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 f | sha256sum -c"
)]
#[case::docker_port("docker run -p 8080:80 nginx")]
#[case::absolute_path("cat /home/u/p/App2/src/Components/UserProfileCard2/Index.tsx")]
#[case::relative_path("vim src/Components/UserProfileCard2/LongFileName2.tsx")]
#[case::url_path("curl https://x.test/Owner/Repo2/blob/main/Src/LongFileName2.rs")]
#[case::kebab_case(
    "cargo test -p efr-conversation naming-rule-ignores-a-name-from-an-older-turn-2"
)]
#[case::snake_case("cargo test naming_rule_ignores_a_name_from_an_older_turn_2")]
#[case::camel_case_without_digits("java com.example.MyVeryLongClassNameForTestingFactory")]
#[case::uuid("efr history 0192f0c1-7a00-7000-8000-000000000001")]
#[case::short_random_word("echo u8jzPde0IgxLd6GncfBAepfJBd0K")]
#[case::not_a_jwt("echo eyJ.x eyJhbGciOiJIUzI1NiJ9 eyJ..x")]
fn a_word_that_looks_ordinary_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

/// The lines of the command corpus of `efr-permissions`.
fn corpus_lines() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Corpus {
        line: Vec<Line>,
    }
    #[derive(serde::Deserialize)]
    struct Line {
        #[serde(default)]
        line: Option<String>,
    }
    const CORPUS: &str =
        include_str!("../../../../efr-permissions/tests/fixtures/auto-corpus.toml");
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    corpus.line.into_iter().filter_map(|line| line.line).collect()
}

/// The rules for the keys of other services, random words and web tokens (v0.0.4) find
/// nothing in the normal command lines of the corpus.
#[test]
fn the_key_rules_find_no_key_in_the_command_corpus() {
    let lines = corpus_lines();
    assert!(lines.len() > 100, "{}", lines.len());
    for line in &lines {
        let found: Vec<std::ops::Range<usize>> =
            super::random_words(line).into_iter().chain(super::web_tokens(line)).collect();
        assert!(found.is_empty(), "{line}: {found:?}");
        let keys: Vec<&str> = super::keys(line).into_iter().map(|range| &line[range]).collect();
        assert!(
            keys.iter().all(|key| !["AKIA", "AIza", "npm_", "hf_", "glpat-", "xox", "pypi-"]
                .iter()
                .any(|prefix| key.starts_with(prefix))),
            "{line}: {keys:?}"
        );
    }
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
#[case::bundled_option(
    "curl -sH 'Authorization: Bearer abc' x",
    "curl -sH 'Authorization: Bearer [redacted]' x"
)]
#[case::echo_quoted(
    r#"echo "Authorization: Bearer abc" > h.txt"#,
    r#"echo "Authorization: Bearer [redacted]" > h.txt"#
)]
#[case::xh("xh x X-Api-Key:abc", "xh x X-Api-Key:[redacted]")]
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
#[case::grep_scheme(r#"grep -rn "Authorization: Bearer" ."#)]
#[case::rg_value(r#"rg "password: true" config.yml"#)]
#[case::echo_other_header(r#"echo "Token: done""#)]
#[case::echo_other_header_unquoted("echo Token: done")]
#[case::scheme_alone("curl -H 'Authorization: Bearer' x")]
#[case::scheme_alone_unquoted("echo Authorization: Bearer")]
fn a_header_without_a_secret_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[rstest]
#[case::mysql("mysql -u root -phunter2 app", "mysql -u root -p[redacted] app")]
#[case::mysql_quoted("mysql -uroot -p'a b' app", "mysql -uroot -p'[redacted]' app")]
#[case::mysqldump("mysqldump -pabc app > dump.sql", "mysqldump -p[redacted] app > dump.sql")]
#[case::mariadb_long("mariadb --password=abc", "mariadb --password=[redacted]")]
#[case::sudo("sudo -u mysql mysql -pabc", "sudo -u mysql mysql -p[redacted]")]
#[case::env("env -i A=1 mysql -pabc", "env -i A=1 mysql -p[redacted]")]
#[case::path_of_the_program("/usr/bin/mysql -pabc", "/usr/bin/mysql -p[redacted]")]
#[case::after_an_assignment("LANG=C mysql -pabc", "LANG=C mysql -p[redacted]")]
#[case::timeout("timeout -s KILL 5 mysql -pabc", "timeout -s KILL 5 mysql -p[redacted]")]
#[case::second_command("cd x && mysql -pabc", "cd x && mysql -p[redacted]")]
#[case::sshpass("sshpass -p abc ssh host", "sshpass -p [redacted] ssh host")]
#[case::sshpass_attached("sshpass -pabc ssh host", "sshpass -p[redacted] ssh host")]
#[case::docker_login(
    "docker login -u me -p abc registry.io",
    "docker login -u me -p [redacted] registry.io"
)]
#[case::docker_login_long(
    "docker login --password abc registry.io",
    "docker login --password [redacted] registry.io"
)]
#[case::docker_login_after_a_global_option(
    "docker --config d login -p abc registry.io",
    "docker --config d login -p [redacted] registry.io"
)]
#[case::helm_registry_login(
    "helm registry login r.io -u me -p abc",
    "helm registry login r.io -u me -p [redacted]"
)]
#[case::helm_repo(
    "helm repo add r https://x --username u --password abc",
    "helm repo add r https://x --username u --password [redacted]"
)]
#[case::redis("redis-cli -h h -a abc ping", "redis-cli -h h -a [redacted] ping")]
#[case::mongosh("mongosh -u admin -p abc", "mongosh -u admin -p [redacted]")]
#[case::curl_user("curl -u me:abc https://x", "curl -u me:[redacted] https://x")]
#[case::curl_user_attached("curl -ume:abc https://x", "curl -ume:[redacted] https://x")]
#[case::curl_user_long("curl --user 'me:a b' https://x", "curl --user 'me:[redacted]' https://x")]
#[case::curl_bundled("curl -su me:abc https://x", "curl -su me:[redacted] https://x")]
#[case::curl_bundled_long("curl -fsSLu me:abc https://x", "curl -fsSLu me:[redacted] https://x")]
#[case::zip_bundled("zip -rP abc out.zip d", "zip -rP [redacted] out.zip d")]
#[case::ldap_bundled("ldapsearch -xw abc -b dc=x", "ldapsearch -xw [redacted] -b dc=x")]
#[case::zip("zip -P abc out.zip f", "zip -P [redacted] out.zip f")]
#[case::seven_zip("7z a -pabc out.7z f", "7z a -p[redacted] out.7z f")]
#[case::ldap("ldapsearch -D cn=me -w abc", "ldapsearch -D cn=me -w [redacted]")]
#[case::openssl(
    "openssl rsa -in k.pem -passin pass:abc",
    "openssl rsa -in k.pem -passin pass:[redacted]"
)]
#[case::openssl_key("openssl enc -aes-256-cbc -k abc", "openssl enc -aes-256-cbc -k [redacted]")]
#[case::wget("wget --http-password abc x", "wget --http-password [redacted] x")]
#[case::gpg("gpg --batch --passphrase abc -d f", "gpg --batch --passphrase [redacted] -d f")]
#[case::kubectl("kubectl --token abc get pods", "kubectl --token [redacted] get pods")]
fn the_value_of_a_password_option_is_redacted(#[case] line: &str, #[case] redacted: &str) {
    assert_eq!(redact(line), redacted);
}

#[rstest]
#[case::mysql_asks("mysql -u root -p app")]
#[case::mysql_asks_last("mysql -u root -p")]
#[case::mysql_port("mysql -P 3306 -h db")]
#[case::mysql_long_asks("mysql --password app")]
#[case::psql_asks("psql --password -d app")]
#[case::docker_run("docker run -p 8080:80 nginx")]
#[case::docker_run_with_login_later("docker run -p 8080:80 nginx login")]
#[case::option_before_login("docker -p x login")]
#[case::podman_port("podman run -p 80:80 img")]
#[case::other_program("grep -p abc file")]
#[case::ssh_port("ssh -p 2222 host")]
#[case::scp_port("scp -P 2222 f host:")]
#[case::mongosh_asks("mongosh -u admin -p --authenticationDatabase admin")]
#[case::curl_user_asks("curl -u me https://x")]
#[case::curl_agent("curl --user-agent 'x:y' https://x")]
#[case::curl_bundle_without_user("curl -fsSL https://x:8080/a")]
#[case::bundle_of_another_program("ssh -tp 22 host")]
#[case::openssl_env("openssl rsa -in k.pem -passin env:KEY")]
#[case::reference(r#"mysql -p"$MYSQL_PWD" app"#)]
#[case::reference_next("sshpass -p $PASS ssh host")]
#[case::after_double_dash("sshpass -- -p abc")]
#[case::program_as_argument("echo mysql -pabc")]
#[case::stdin("docker login -u me --password-stdin registry.io")]
fn an_option_that_takes_no_password_here_stays(#[case] line: &str) {
    assert_eq!(redact(line), line);
}

#[test]
fn secrets_in_several_forms_on_one_line_each_go_once() {
    let line = "A_TOKEN=x curl -u me:pw -H 'Authorization: Bearer sk-proj-ABCDEFGHIJKLMNOPQRST' \
                https://u:p@h && mysql -pq";
    assert_eq!(
        redact(line),
        "A_TOKEN=[redacted] curl -u me:[redacted] -H 'Authorization: Bearer [redacted]' \
         https://u:[redacted]@h && mysql -p[redacted]"
    );
}

/// Parts of lines that end a pattern early or late: quotes that do not close, a
/// backslash at the end, characters outside ASCII next to the characters that the
/// patterns look for.
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

/// A fixed corpus of 100 lines: commands, prose, keys of each form and keys cut at
/// the end of a line. Its snapshot shows each line with its redaction, so a change of
/// the output shows in review.
const REDACTION_CORPUS: &[&str] = &[
    "ls -la",
    "cd ~/p/eggfriedrice.code && cargo test -p efr-store",
    "git log --oneline -5",
    "git show 3f786850e387550fdab836ed7e6dc881de23001b",
    "docker run -p 8080:80 nginx",
    "rsync -av ./site/ laptop:/srv/site/",
    "grep -rn \"password:\" config/",
    "rg \"Authorization: Bearer\" src",
    "export GITHUB_TOKEN=ghp_0123456789abcdefABCDEF0123456789abcd",
    "export GITHUB_TOKEN=$OTHER",
    "API_KEY=abc123 cargo run",
    "export DB_PASSWORD=\"a b c\" && psql",
    "AWS_SECRET_ACCESS_KEY='x y'; aws s3 ls",
    "MY_TOKEN=$'a\\tb' run",
    "tool --api-key=abc --verbose",
    "curl -d \"user=me&password=hunter2\" x",
    "TOKEN=\"abc def",
    "TOKEN=abc\\ def cmd",
    "A_TOKEN=1 B_SECRET=2 PATH=/bin",
    "kubectl create secret generic s --from-literal=password=hunter2",
    "docker run --env=API_TOKEN=abc123 img",
    "helm install x --set=auth.password=abc ./chart",
    "llm --max-tokens=4096",
    "TOKENIZERS_PARALLELISM=false python x.py",
    "make TOKEN_COUNT=5",
    "curl -H 'x-api-key: sk-ant-api03-abcdefghijklmnopqrstuv' x",
    "echo sk-proj-ABCDEFGHIJKLMNOPQRST_123 | pbcopy",
    "echo sk-0123456789abcdefABCDEF",
    "git remote set-url origin https://github_pat_11ABCDEFG0123456789_abcdef@github.com/o/r",
    "echo glpat-0123456789abcdefABCD",
    "echo pypi-AgEIcHlwaS5vcmcCJDAwMDAwMDAw",
    "echo xoxb-1234-5678-Db_xHKas1VOqg6YYZYn9 | x",
    "echo xoxp-1234-5678-Db_xHKas1VOqg6YYZYn9",
    "echo ghp_0123456789abcdefABCDEF0123456789abcd",
    "echo gho_0123456789abcdefABCDEF0123456789abcd",
    "echo ghu_0123456789abcdefABCDEF0123456789abcd",
    "echo ghs_0123456789abcdefABCDEF0123456789abcd",
    "echo ghr_0123456789abcdefABCDEF0123456789abcd",
    "aws configure set aws_access_key_id AKIAIOSFODNN7EXAMPLE",
    "echo AIzaihA-2O76UMFxFkM-R5Kjp1vRt_1fjORS-6i",
    "echo npm_0123456789abcdefABCDEF0123456789",
    "echo hf_0123456789abcdefABCDEF0123456789ab",
    "pip install sk-learn",
    "git checkout ghp_short",
    "ask-0123456789abcdefABCDEF and task-0123456789abcdefABCDEF",
    "echo $npm_config_userconfig npm_config_registry_of_the_project",
    "echo AKIA1234",
    "echo u8jzPde0IgxLd6GncfBAepfJBd0Kq7Zr2LmN",
    "echo dGhpcyBpcyBhIHRlc3QgT2YgYmFzZTY0IGVuY29kaW5n==",
    "echo wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    "echo eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
    "echo eyJhbGciOiJub25lIn0.eyJzdWIiOiIxIn0.",
    "echo eyJ.x eyJhbGciOiJIUzI1NiJ9 eyJ..x",
    "cat /home/u/p/App2/src/Components/UserProfileCard2/Index.tsx",
    "vim src/Components/UserProfileCard2/LongFileName2.tsx",
    "curl https://x.test/Owner/Repo2/blob/main/Src/LongFileName2.rs",
    "cargo test naming_rule_ignores_a_name_from_an_older_turn_2",
    "efr history 0192f0c1-7a00-7000-8000-000000000001",
    "git clone https://me:hunter2@github.com/o/r",
    "psql postgres://app:s3cret@db:5432/app",
    "curl http://localhost:8080/api",
    "git clone ssh://git@github.com:22/o/r",
    "git clone https://me:$TOKEN@github.com/o/r",
    "curl -H 'Authorization: Bearer abc123' https://api.example",
    "curl -H 'Authorization: Basic dXNlcjpwYXNz' x",
    "curl -H 'Cookie: a=1; session=abc' x",
    "curl -H \"Authorization: Bearer $TOKEN\" x",
    "curl -H 'Content-Type: application/json' x",
    "http POST api.example X-Api-Key:abc",
    "echo Authorization: Bearer abc && ls",
    "git -c http.extraHeader='Authorization: token abc' fetch",
    "mysql -u root -phunter2 app",
    "mysql -u root -p app",
    "sshpass -p abc ssh host",
    "docker login -u me -p abc registry.example",
    "redis-cli -h h -a abc ping",
    "curl -u me:abc https://x",
    "zip -P abc out.zip f",
    "openssl enc -aes-256-cbc -k abc",
    "gpg --batch --passphrase abc -d f",
    "kubectl --token abc get pods",
    "my password is hunter2, please keep it safe",
    "the token in the config is 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    "password: hunter2xyz",
    "api_key = \"abc123\"",
    "\"password\": \"hunter2\"",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW",
    "Use `cargo nextest run` to run the tests, then open a PR.",
    "Le mot de passe est écrit dans le fichier, PASSWORD=ü ✓",
    "the key ends the line: sk-ant-api03-abcdefghij",
    "the key ends the line: sk-ant-api03-abcdefghijklmnop",
    "the key ends the line: ghp_0123456789abcdef",
    "TOKEN=abc",
    "x=1,y=2,z=3",
    "echo a=\"a=\"a=\"",
    "for f in *.rs; do wc -l \"$f\"; done",
    "cat > .env <<'EOF'",
    "ssh -i ~/.ssh/id_ed25519 me@host 'uptime'",
    "notify-send done",
];

/// The redaction of the fixed corpus, line by line, under the version of the index
/// redaction: a change of the output that keeps the version is a missing increase of
/// `REDACTION_VERSION`.
#[test]
fn redaction_corpus_is_stable() {
    assert_eq!(REDACTION_CORPUS.len(), 100);
    let mut out = format!("REDACTION_VERSION {}\n", crate::REDACTION_VERSION);
    for line in REDACTION_CORPUS {
        out.push_str(line);
        out.push_str("\n=> ");
        out.push_str(&redact(line));
        out.push('\n');
    }
    insta::assert_snapshot!(out);
}
