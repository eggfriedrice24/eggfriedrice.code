use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Env, Var};
use crate::StdxError;

#[test]
fn unset_var_is_none() {
    let env = Env::fixed::<_, &str>([]);
    assert_eq!(env.var(Var::Log).unwrap(), None);
}

#[test]
fn empty_var_counts_as_unset() {
    let env = Env::fixed([(Var::Log, "")]);
    assert_eq!(env.var(Var::Log).unwrap(), None);
}

#[test]
fn set_var_is_returned() {
    let env = Env::fixed([(Var::Log, "info,efr_=debug")]);
    assert_eq!(env.var(Var::Log).unwrap().as_deref(), Some("info,efr_=debug"));
}

#[test]
fn var_reads_only_its_own_name() {
    let env = Env::fixed([(Var::Screen, "vt100")]);
    assert_eq!(env.var(Var::Log).unwrap(), None);
}

#[test]
fn non_utf8_var_is_an_error() {
    let env = Env::fixed([(Var::Log, OsString::from_vec(vec![b'a', 0xff]))]);
    let err = env.var(Var::Log).unwrap_err();
    assert!(matches!(err, StdxError::NotUnicode { var: Var::Log }), "{err:?}");
}

#[test]
fn absolute_path_is_returned() {
    let env = Env::fixed([(Var::DataDir, "/tmp/efr-data")]);
    assert_eq!(env.path(Var::DataDir).unwrap(), Some(PathBuf::from("/tmp/efr-data")));
}

#[test]
fn path_may_hold_non_utf8_bytes() {
    let raw = vec![b'/', b'd', 0xff];
    let env = Env::fixed([(Var::DataDir, OsString::from_vec(raw.clone()))]);
    assert_eq!(env.path(Var::DataDir).unwrap(), Some(PathBuf::from(OsString::from_vec(raw))));
}

#[test]
fn relative_path_is_an_error() {
    let env = Env::fixed([(Var::DataDir, "data")]);
    let err = env.path(Var::DataDir).unwrap_err();
    match err {
        StdxError::RelativeEnvPath { var, path } => {
            assert_eq!(var, Var::DataDir);
            assert_eq!(path, PathBuf::from("data"));
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn unset_path_is_none() {
    let env = Env::fixed([(Var::DataDir, "")]);
    assert_eq!(env.path(Var::DataDir).unwrap(), None);
}

#[rstest]
#[case("1", true)]
#[case("true", true)]
#[case("TRUE", true)]
#[case("yes", true)]
#[case("On", true)]
#[case("0", false)]
#[case("false", false)]
#[case("No", false)]
#[case("off", false)]
#[case("", false)]
fn flag_values(#[case] value: &str, #[case] expected: bool) {
    let env = Env::fixed([(Var::TestZsh, value)]);
    assert_eq!(env.flag(Var::TestZsh).unwrap(), expected);
}

#[test]
fn unset_flag_is_off() {
    let env = Env::fixed::<_, &str>([]);
    assert!(!env.flag(Var::OpenBrowser).unwrap());
}

#[rstest]
#[case("2")]
#[case("y")]
#[case(" 1")]
#[case("enabled")]
fn invalid_flag_is_an_error(#[case] value: &str) {
    let env = Env::fixed([(Var::OpenBrowser, value)]);
    match env.flag(Var::OpenBrowser).unwrap_err() {
        StdxError::InvalidEnvValue { var, value: got, .. } => {
            assert_eq!(var, Var::OpenBrowser);
            assert_eq!(got, value);
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn display_is_the_name() {
    assert_eq!(Var::RuntimeDir.to_string(), "EFR_RUNTIME_DIR");
}

#[test]
fn names_are_unique_and_prefixed() {
    let names: BTreeSet<_> = Var::ALL.iter().map(|var| var.name()).collect();
    assert_eq!(names.len(), Var::ALL.len());
    for name in names {
        assert!(name.starts_with("EFR_"), "{name} has no EFR_ prefix");
    }
}

#[test]
fn all_lists_every_variant() {
    // The match is exhaustive, so a new variant fails to compile here; adding it to
    // this list then makes the assertion check that `Var::ALL` has it too.
    let every = [
        Var::Log,
        Var::Screen,
        Var::ConfigDir,
        Var::DataDir,
        Var::StateDir,
        Var::RuntimeDir,
        Var::OpenBrowser,
        Var::RecordTranscript,
        Var::TestZsh,
    ];
    for var in every {
        match var {
            Var::Log
            | Var::Screen
            | Var::ConfigDir
            | Var::DataDir
            | Var::StateDir
            | Var::RuntimeDir
            | Var::OpenBrowser
            | Var::RecordTranscript
            | Var::TestZsh => assert!(Var::ALL.contains(&var), "{var} is missing from Var::ALL"),
        }
    }
    assert_eq!(every.len(), Var::ALL.len());
}

#[test]
fn readme_documents_exactly_these_variables() {
    let readme = include_str!("../../README.md");
    let documented = efr_names(readme);
    let known: BTreeSet<String> = Var::ALL.iter().map(|var| var.name().to_owned()).collect();
    assert_eq!(documented, known);
}

/// Every `EFR_NAME` token in `text`; a bare `EFR_` (as in `EFR_*`) is not a name.
fn efr_names(text: &str) -> BTreeSet<String> {
    text.match_indices("EFR_")
        .filter_map(|(at, prefix)| {
            let rest = &text[at + prefix.len()..];
            let len = rest
                .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(rest.len());
            (len > 0).then(|| format!("{prefix}{}", &rest[..len]))
        })
        .collect()
}
