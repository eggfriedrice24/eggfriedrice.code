use efr_protocol::{ApprovedBy, InputWait, ShellCommandEnd};
use pretty_assertions::assert_eq;

use super::{approver, ended, waits_for};

#[test]
fn each_end_has_its_own_words() {
    let notes: Vec<String> = ShellCommandEnd::ALL
        .into_iter()
        .map(|how| ended("rsync -a x y", Some(1), how, false))
        .collect();
    assert_eq!(
        notes,
        [
            "rsync -a x y ended in the hidden shell with exit 1",
            "rsync -a x y stopped in the hidden shell",
            "efr stopped rsync -a x y in the hidden shell: nobody answered its input in time",
            "the hidden shell closed with rsync -a x y in it; the next call starts a new shell",
            "the hidden shell exited with rsync -a x y in it",
            "rsync -a x y moved to a background job",
            "rsync -a x y no longer runs in the hidden shell",
        ]
    );
    assert_eq!(
        ended("make", Some(0), ShellCommandEnd::Ended, false),
        "make ended in the hidden shell"
    );
}

#[test]
fn a_secret_stop_in_the_sandbox_is_no_limit_and_names_no_password() {
    assert_eq!(
        ended("ssh h", None, ShellCommandEnd::Limit, true),
        "efr stopped ssh h in the hidden shell: it asked for a secret in the sandbox, where \
         efr types none"
    );
    assert_eq!(
        ended("ssh h", Some(0), ShellCommandEnd::Ended, true),
        "ssh h ended in the hidden shell"
    );
}

#[test]
fn a_password_is_named_only_outside_the_sandbox() {
    assert_eq!(waits_for(InputWait::Hidden, false, false), Some("a password"));
    assert_eq!(waits_for(InputWait::Visible, true, false), Some("a password"));
    assert_eq!(waits_for(InputWait::Visible, false, false), Some("input"));
    assert_eq!(waits_for(InputWait::Hidden, false, true), Some("input"));
    assert_eq!(waits_for(InputWait::None, false, false), None);
}

#[test]
fn every_known_approver_has_words() {
    for approved_by in ApprovedBy::ALL {
        assert_eq!(approver(approved_by).is_some(), approved_by != ApprovedBy::Unknown);
    }
}
