use efr_protocol::{ApprovedBy, InputWait, ShellCommandEnd};
use pretty_assertions::assert_eq;

use super::{approver, ended, waits_for};

#[test]
fn each_end_has_its_own_words() {
    let notes: Vec<String> =
        ShellCommandEnd::ALL.into_iter().map(|how| ended("rsync -a x y", Some(1), how)).collect();
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
    assert_eq!(ended("make", Some(0), ShellCommandEnd::Ended), "make ended in the hidden shell");
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
