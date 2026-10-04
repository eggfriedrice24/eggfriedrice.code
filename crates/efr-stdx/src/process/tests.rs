use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRUBBED_ENV, command};
use crate::env::Var;

/// The environment changes a command makes: `Some` sets a variable, `None` removes it.
fn env_changes(command: &tokio::process::Command) -> BTreeMap<String, Option<String>> {
    command
        .as_std()
        .get_envs()
        .map(|(name, value)| {
            let name = name.to_string_lossy().into_owned();
            (name, value.map(|value| value.to_string_lossy().into_owned()))
        })
        .collect()
}

#[test]
fn command_runs_the_program_in_the_given_directory() {
    let command = command("zsh", "/srv/project");
    assert_eq!(command.as_std().get_program(), OsStr::new("zsh"));
    assert_eq!(command.as_std().get_current_dir(), Some(Path::new("/srv/project")));
}

#[rstest]
#[case("NOTIFY_SOCKET")]
#[case("LISTEN_FDS")]
#[case("LISTEN_PID")]
#[case("LISTEN_FDNAMES")]
#[case("WATCHDOG_USEC")]
#[case("WATCHDOG_PID")]
#[case("JOURNAL_STREAM")]
fn systemd_unit_variables_are_removed(#[case] name: &str) {
    assert!(SCRUBBED_ENV.contains(&name));
    let changes = env_changes(&command("true", "/"));
    assert_eq!(changes.get(name), Some(&None), "{name} is not removed");
}

#[test]
fn what_the_plugin_hands_to_efr_never_reaches_a_child() {
    let changes = env_changes(&command("true", "/"));
    for name in ["EFR_CONTEXT", "EFR_LAST_COMMAND", "EFR_PROMPT"] {
        assert_eq!(changes.get(name), Some(&None), "{name} is not removed");
    }
}

#[test]
fn scrub_list_has_no_duplicates() {
    let mut names = SCRUBBED_ENV.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SCRUBBED_ENV.len());
}

#[test]
fn absolute_cwd_becomes_pwd() {
    let changes = env_changes(&command("true", "/srv/project"));
    assert_eq!(changes.get("PWD"), Some(&Some("/srv/project".to_owned())));
}

#[test]
fn relative_cwd_removes_pwd() {
    let changes = env_changes(&command("true", "project"));
    assert_eq!(changes.get("PWD"), Some(&None));
}

#[test]
fn other_variables_are_inherited() {
    let changes = env_changes(&command("true", "/"));
    let expected: BTreeMap<_, _> = SCRUBBED_ENV
        .iter()
        .copied()
        .chain(Var::PRIVATE.iter().map(|var| var.name()))
        .map(|name| (name.to_owned(), None))
        .chain([("PWD".to_owned(), Some("/".to_owned()))])
        .collect();
    assert_eq!(changes, expected);
}

#[tokio::test]
async fn child_starts_in_cwd_with_a_matching_pwd() {
    let dir = tempfile::tempdir().unwrap();
    let output = command("sh", dir.path())
        .args(["-c", r#"pwd -P; printf '%s\n' "$PWD""#])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    let expected = format!("{}\n{}\n", canonical.display(), dir.path().display());
    assert_eq!(stdout, expected);
}
