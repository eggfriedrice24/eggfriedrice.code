use pretty_assertions::assert_eq;
use rstest::rstest;

use super::AllowRule;
use crate::{AllowRuleProblem, PermissionsError};

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    allow: Vec<AllowRule>,
}

fn host(host: &str, port: u16) -> AllowRule {
    AllowRule::Host { host: host.to_owned(), port }
}

fn write(under: &str, project: Option<&str>) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: project.map(str::to_owned) }
}

fn socket(path: &str) -> AllowRule {
    AllowRule::Socket { path: path.to_owned() }
}

#[test]
fn allow_rule_shapes_parse_and_refuse_unknown_fields() {
    let file: File = toml::from_str(
        r#"
[[allow]]
kind = "host"
host = "download.example.org"

[[allow]]
kind = "host"
host = "mirror.example.org"
port = 8443

[[allow]]
kind = "write"
under = "~/notes"

[[allow]]
kind = "write"
under = "~/p/app/out"
project = "~/p/app"

[[allow]]
kind = "socket"
path = "/run/user/1000/mpd/socket"
"#,
    )
    .unwrap();
    assert_eq!(
        file.allow,
        [
            host("download.example.org", 443),
            host("mirror.example.org", 8443),
            write("~/notes", None),
            write("~/p/app/out", Some("~/p/app")),
            socket("/run/user/1000/mpd/socket"),
        ]
    );

    for (text, word) in [
        ("kind = \"host\"\nhost = \"a.example.org\"\nunder = \"~/x\"", "under"),
        ("kind = \"write\"\nunder = \"~/x\"\nhost = \"a.example.org\"", "host"),
        ("kind = \"socket\"\npath = \"/run/x\"\nport = 1", "port"),
        ("kind = \"dbus\"\nbus = \"system\"", "dbus"),
        ("kind = \"device\"\npath = \"/dev/x\"", "device"),
    ] {
        let error = toml::from_str::<AllowRule>(text).unwrap_err().to_string();
        assert!(error.contains(word), "{text}: {error}");
    }
    let error = toml::from_str::<AllowRule>("host = \"a.example.org\"").unwrap_err().to_string();
    assert!(error.contains("kind"), "{error}");
}

#[test]
fn allow_rule_json_schema_names_each_kind() {
    let schema = schemars::schema_for!(AllowRule);
    let schema = schema.as_value();
    let text = schema.to_string();
    for word in ["\"host\"", "\"write\"", "\"socket\"", "\"under\"", "\"project\"", "\"port\""] {
        assert!(text.contains(word), "{word} in {text}");
    }
    let variants = schema["oneOf"].as_array().expect("one schema per kind");
    assert_eq!(variants.len(), 3);
    for variant in variants {
        assert_eq!(variant["additionalProperties"].as_bool(), Some(false), "{variant}");
        let required = variant["required"].as_array().unwrap();
        assert!(required.iter().any(|word| word.as_str() == Some("kind")), "{variant}");
    }
}

#[test]
fn allow_rule_describe() {
    assert_eq!(host("download.example.org", 443).describe(), "host download.example.org:443");
    assert_eq!(write("~/notes", None).describe(), "write under ~/notes");
    assert_eq!(
        write("~/p/app/out", Some("~/p/app")).describe(),
        "write under ~/p/app/out in project ~/p/app"
    );
    assert_eq!(socket("/run/user/1000/mpd/socket").describe(), "socket /run/user/1000/mpd/socket");
}

#[rstest]
#[case(host("download.example.org", 443))]
#[case(host("Download.Example.ORG.", 80))]
#[case(host("intranet", 8080))]
#[case(write("~/notes", None))]
#[case(write("~", None))]
#[case(write("/srv/data", Some("~/p/app")))]
#[case(socket("/run/user/1000/mpd/socket"))]
#[case(socket("~/.local/run/app.sock"))]
fn allow_rule_shape_accepts(#[case] rule: AllowRule) {
    assert_eq!(rule.check_shape(0), Ok(()));
}

#[rstest]
#[case(host("*.example.org", 443), AllowRuleProblem::HostPattern { host: "*.example.org".to_owned() })]
#[case(host("*", 443), AllowRuleProblem::HostPattern { host: "*".to_owned() })]
#[case(host("10.0.0.1", 443), AllowRuleProblem::IpLiteral { host: "10.0.0.1".to_owned() })]
#[case(host("[::1]", 443), AllowRuleProblem::IpLiteral { host: "[::1]".to_owned() })]
#[case(host("localhost", 443), AllowRuleProblem::Localhost { host: "localhost".to_owned() })]
#[case(host("app.localhost", 443), AllowRuleProblem::Localhost { host: "app.localhost".to_owned() })]
#[case(host("bad_name.example.org", 443), AllowRuleProblem::BadHost { host: "bad_name.example.org".to_owned() })]
#[case(host("bücher.example", 443), AllowRuleProblem::BadHost { host: "bücher.example".to_owned() })]
#[case(host("", 443), AllowRuleProblem::BadHost { host: String::new() })]
#[case(host("example.org", 0), AllowRuleProblem::PortZero)]
#[case(write("notes", None), AllowRuleProblem::BadPath { path: "notes".to_owned() })]
#[case(write("", None), AllowRuleProblem::BadPath { path: String::new() })]
#[case(write("~user/notes", None), AllowRuleProblem::BadPath { path: "~user/notes".to_owned() })]
#[case(write("~/notes/../.ssh", None), AllowRuleProblem::DotDot { path: "~/notes/../.ssh".to_owned() })]
#[case(write("/srv/./data", None), AllowRuleProblem::DotDot { path: "/srv/./data".to_owned() })]
#[case(write("~/notes", Some("p/app")), AllowRuleProblem::BadPath { path: "p/app".to_owned() })]
#[case(socket("run/x.sock"), AllowRuleProblem::BadPath { path: "run/x.sock".to_owned() })]
#[case(socket("/run/../x.sock"), AllowRuleProblem::DotDot { path: "/run/../x.sock".to_owned() })]
fn allow_rule_shape_refuses(#[case] rule: AllowRule, #[case] problem: AllowRuleProblem) {
    assert_eq!(rule.check_shape(3), Err(PermissionsError::AllowRule { index: 3, problem }));
}

#[test]
fn allow_rule_error_names_the_index_and_the_reason() {
    let error = host("*.example.org", 443).check_shape(2).unwrap_err();
    assert_eq!(error.to_string(), "sandbox.allow[2] is refused");
    let source = std::error::Error::source(&error).map(ToString::to_string);
    assert_eq!(
        source.as_deref(),
        Some("the host \"*.example.org\" is a pattern; a rule names one host")
    );
}
