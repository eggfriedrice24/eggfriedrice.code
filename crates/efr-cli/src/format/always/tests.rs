use efr_protocol::{AlwaysSaved, NotSavedReason};
use pretty_assertions::assert_eq;

use super::{covered, disabled, list_row, remove_question, saved};

#[test]
fn saved_says_where_the_rule_is() {
    let outcome = AlwaysSaved::Saved { index: 2, rule: "write under ~/notes".to_owned() };
    assert_eq!(saved(&outcome), "saved as sandbox.allow[2]: write under ~/notes");
    // The rule comes from the daemon: it cannot drive the terminal.
    let outcome = AlwaysSaved::Saved { index: 0, rule: "host a.test:443\x1b[2J".to_owned() };
    assert_eq!(saved(&outcome), "saved as sandbox.allow[0]: host a.test:443\u{241b}[2J");
    assert_eq!(
        saved(&AlwaysSaved::Unknown),
        "efr cannot tell if the rule was saved; efr sandbox allow lists the rules"
    );
}

#[test]
fn not_saved_says_why_and_that_the_command_ran() {
    let line = |reason| saved(&AlwaysSaved::NotSaved { reason });
    assert_eq!(
        line(NotSavedReason::ReadOnly),
        "rule not saved: config.toml is read-only; the command ran once"
    );
    assert_eq!(
        line(NotSavedReason::Changed),
        "rule not saved: config.toml changed each time efr tried to write it; the command ran \
         once"
    );
    assert_eq!(
        line(NotSavedReason::LoadCheck),
        "rule not saved: the rule did not pass the floor checks; the command ran once"
    );
    assert_eq!(
        line(NotSavedReason::Io),
        "rule not saved: efr could not read or write config.toml; the command ran once"
    );
    assert_eq!(
        line(NotSavedReason::Unknown),
        "rule not saved: an unknown reason; the command ran once"
    );
    // The rule was written; only the reload was late.
    assert_eq!(
        line(NotSavedReason::ReloadTimeout),
        "the rule is in config.toml, but efrd did not read the file again in time; a later \
         command can ask once more"
    );
}

#[test]
fn covered_names_the_rule_and_its_place() {
    assert_eq!(
        covered(Some("sandbox.allow[2]"), Some("write under ~/notes")),
        "allowed by your rule sandbox.allow[2]: write under ~/notes"
    );
    assert_eq!(covered(Some("sandbox.allow[2]"), None), "allowed by your rule sandbox.allow[2]");
    assert_eq!(covered(None, Some("host a.test:443")), "allowed by your rule: host a.test:443");
    assert_eq!(covered(Some(" "), Some("")), "allowed by your rule");
}

#[test]
fn disabled_rules_come_from_the_sandbox_warnings() {
    let warnings = [
        "the network view's self-test failed".to_owned(),
        "sandbox.allow[1] is disabled: it names a floor".to_owned(),
        "sandbox.allow[x] is disabled: not a number".to_owned(),
        "sandbox.allow[12] is disabled: it is a synced dir".to_owned(),
    ];
    assert_eq!(
        disabled(&warnings),
        [(1, "it names a floor".to_owned()), (12, "it is a synced dir".to_owned())]
    );
}

#[test]
fn a_list_row_and_the_remove_question_name_the_rule_safely() {
    assert_eq!(list_row(0, "write under ~/notes", None), "sandbox.allow[0]  write under ~/notes");
    assert_eq!(
        list_row(1, "socket ~/a\x1b[2J", Some("it is a link\n")),
        "sandbox.allow[1]  socket ~/a\u{241b}[2J; disabled: it is a link "
    );
    assert_eq!(
        remove_question(2, "host a.test:443"),
        "remove sandbox.allow[2]: host a.test:443? y/n"
    );
}
