use std::path::Path;

use efr_protocol::{PtyId, Size};
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

use super::SpawnSpec;
use crate::HolderError;

const PTY: &str = "01920000-0000-7000-8000-000000000001";

fn pty_id() -> PtyId {
    PTY.parse().unwrap()
}

fn size() -> Size {
    Size { cols: 80, rows: 24 }
}

fn zsh() -> SpawnSpec {
    SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/user", size())
}

#[test]
fn setters_append_arguments_and_replace_variables() {
    let spec = zsh()
        .arg("-i")
        .args(["-o", "no_rcs"])
        .var("TERM", "dumb")
        .vars([("TERM", "xterm-256color"), ("ZDOTDIR", "/run/efr/zsh")]);
    assert_eq!(spec.args, ["-i", "-o", "no_rcs"]);
    assert_eq!(
        spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(),
        [("TERM", "xterm-256color"), ("ZDOTDIR", "/run/efr/zsh")],
    );
}

#[test]
fn a_full_spec_is_valid() {
    let spec = zsh().arg("-i").var("TERM", "xterm-256color").var("EMPTY", "");
    assert!(spec.validate().is_ok());
}

#[test]
fn a_one_cell_terminal_is_valid() {
    let spec = SpawnSpec::new(pty_id(), "/bin/sh", "/", Size { cols: 1, rows: 1 });
    assert!(spec.validate().is_ok());
}

/// Each case breaks exactly one rule and names the variant it must produce.
#[rstest]
#[case::relative_program(
    SpawnSpec::new(pty_id(), "zsh", "/home/user", size()),
    "the program zsh is not an absolute path"
)]
#[case::empty_program(
    SpawnSpec::new(pty_id(), "", "/home/user", size()),
    "the program  is not an absolute path"
)]
#[case::nul_in_program(
    SpawnSpec::new(pty_id(), "/usr/bin/zsh\0", "/home/user", size()),
    "the program of the spawn spec contains a NUL byte"
)]
#[case::nul_in_argument(zsh().arg("-i").arg("a\0b"), "the argument of the spawn spec contains a NUL byte")]
#[case::relative_cwd(
    SpawnSpec::new(pty_id(), "/usr/bin/zsh", "project", size()),
    "the working directory project is not an absolute path"
)]
#[case::nul_in_cwd(
    SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/\0user", size()),
    "the cwd of the spawn spec contains a NUL byte"
)]
#[case::empty_env_name(zsh().var("", "x"), r#""" is not a valid environment variable name"#)]
#[case::equals_in_env_name(zsh().var("A=B", "x"), r#""A=B" is not a valid environment variable name"#)]
#[case::nul_in_env_name(zsh().var("A\0", "x"), r#""A\0" is not a valid environment variable name"#)]
#[case::nul_in_env_value(
    zsh().var("OPENAI_API_KEY", "sk-secret\0"),
    "the value of the environment variable OPENAI_API_KEY contains a NUL byte"
)]
#[case::zero_columns(
    SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/user", Size { cols: 0, rows: 24 }),
    "the terminal size 0x24 has no cells"
)]
#[case::zero_rows(
    SpawnSpec::new(pty_id(), "/usr/bin/zsh", "/home/user", Size { cols: 80, rows: 0 }),
    "the terminal size 80x0 has no cells"
)]
fn validate_rejects(#[case] spec: SpawnSpec, #[case] message: &str) {
    let error = spec.validate().unwrap_err();
    assert_eq!(error.to_string(), message);
}

#[test]
fn validate_reports_the_first_problem_in_field_order() {
    let spec =
        SpawnSpec::new(pty_id(), "zsh", "project", Size { cols: 0, rows: 0 }).var("A=B", "x");
    assert!(matches!(
        spec.validate(),
        Err(HolderError::ProgramNotAbsolute { program }) if program == Path::new("zsh")
    ));

    let spec = SpawnSpec::new(pty_id(), "/bin/zsh", "project", Size { cols: 0, rows: 0 });
    assert!(matches!(spec.validate(), Err(HolderError::CwdNotAbsolute { .. })));
}

#[test]
fn a_rejected_env_value_is_not_in_the_error() {
    let error = zsh().var("TOKEN", "sk-secret\0").validate().unwrap_err();
    assert!(!format!("{error:?}").contains("sk-secret"));
    assert!(!error.to_string().contains("sk-secret"));
}

#[test]
fn debug_shows_env_names_but_not_values() {
    let spec = zsh().var("OPENAI_API_KEY", "sk-secret").var("TERM", "dumb");
    let debug = format!("{spec:?}");
    assert!(debug.contains(r#"env: ["OPENAI_API_KEY", "TERM"]"#), "{debug}");
    assert!(!debug.contains("sk-secret"), "{debug}");
    assert!(!debug.contains("dumb"), "{debug}");
}

#[test]
fn serializes_with_empty_lists_left_out() {
    let value = serde_json::to_value(zsh()).unwrap();
    assert_eq!(
        value,
        json!({
            "pty_id": PTY,
            "program": "/usr/bin/zsh",
            "cwd": "/home/user",
            "size": {"cols": 80, "rows": 24},
        })
    );
}

#[test]
fn round_trips_through_json() {
    let spec = zsh().args(["-i", "-l"]).var("TERM", "xterm-256color");
    let text = serde_json::to_string(&spec).unwrap();
    assert_eq!(
        text,
        format!(
            r#"{{"pty_id":"{PTY}","program":"/usr/bin/zsh","args":["-i","-l"],"cwd":"/home/user","env":{{"TERM":"xterm-256color"}},"size":{{"cols":80,"rows":24}}}}"#
        )
    );
    let back: SpawnSpec = serde_json::from_str(&text).unwrap();
    assert_eq!(back, spec);
}

#[test]
fn decoding_fills_missing_lists_and_ignores_unknown_fields() {
    let spec: SpawnSpec = serde_json::from_value(json!({
        "pty_id": PTY,
        "program": "/usr/bin/zsh",
        "cwd": "/home/user",
        "size": {"cols": 80, "rows": 24},
        "sandbox": {"from": "a later milestone"},
    }))
    .unwrap();
    assert_eq!(spec, zsh());
}
