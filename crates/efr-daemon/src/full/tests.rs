use std::path::PathBuf;

use efr_config::Settings;
use efr_protocol::{FullModeStatus, FullRefusal, Mode};
use pretty_assertions::assert_eq;

use super::{status, warn_default};
use crate::testing::logged;

/// The default settings with `permissions.allow_full` on.
fn allowed() -> Settings {
    let mut settings = Settings::default();
    settings.permissions.allow_full = true;
    settings
}

#[test]
fn full_status_follows_the_gate_key() {
    assert_eq!(
        status(&Settings::default(), false),
        FullModeStatus { allowed: false, refused: None },
        "the gate is off by default"
    );
    assert_eq!(status(&allowed(), false), FullModeStatus { allowed: true, refused: None });
}

#[test]
fn full_status_refuses_a_shell_that_is_no_zsh() {
    for program in ["/bin/bash", "/bin/sh", "/usr/bin/fish"] {
        let mut settings = allowed();
        settings.shell.program = Some(PathBuf::from(program));
        assert_eq!(status(&settings, false).refused, Some(FullRefusal::NoZsh), "{program}");
    }
}

#[test]
fn full_status_accepts_a_zsh_by_path() {
    for program in [None, Some("/usr/bin/zsh"), Some("/opt/zsh-5.9/bin/zsh-5.9")] {
        let mut settings = allowed();
        settings.shell.program = program.map(PathBuf::from);
        assert_eq!(status(&settings, false).refused, None, "{program:?}");
    }
}

#[test]
fn full_status_reports_root_and_undo_off() {
    let mut settings = allowed();
    settings.undo.enabled = false;
    assert_eq!(status(&settings, false).refused, Some(FullRefusal::UndoOff));
    settings.snapshot.enabled = false;
    assert_eq!(status(&settings, false).refused, Some(FullRefusal::SnapshotOff));
    settings.shell.program = Some(PathBuf::from("/bin/bash"));
    assert_eq!(status(&settings, true).refused, Some(FullRefusal::Root), "root comes first");

    // NOTE: a refusal shows also with the gate off, so `efr status` can name it before
    // the user turns the gate on.
    let mut off = Settings::default();
    off.undo.enabled = false;
    assert_eq!(
        status(&off, false),
        FullModeStatus { allowed: false, refused: Some(FullRefusal::UndoOff) }
    );
}

#[test]
fn a_config_default_of_full_is_warned_about_with_its_refusal() {
    let mut settings = allowed();
    settings.permissions.mode = Mode::Full;
    let line = "permissions.mode is full: every turn that names no mode runs commands as you";
    let refused = "permissions.mode is full, but full mode cannot run: efrd runs as root";
    assert!(!logged().contains(line) && !logged().contains(refused));

    warn_default(&Settings::default(), false);
    assert!(!logged().contains("permissions.mode is full"), "no line without the default");
    warn_default(&settings, false);
    warn_default(&settings, true);

    let log = logged();
    let warned =
        log.lines().find(|logged| logged.contains(line)).unwrap_or_else(|| panic!("{log}"));
    assert!(warned.contains("WARN"), "{warned}");
    let refusal =
        log.lines().find(|logged| logged.contains(refused)).unwrap_or_else(|| panic!("{log}"));
    assert!(refusal.contains("turns that name no mode run as auto"), "{refusal}");
    assert!(refusal.contains("refusal=\"root\"") || refusal.contains("refusal=root"), "{refusal}");
}
