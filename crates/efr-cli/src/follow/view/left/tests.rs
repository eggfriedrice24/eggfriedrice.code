use efr_protocol::{ApprovedBy, InputWait, ShellCommandEnd};
use pretty_assertions::assert_eq;

use super::{Left, ended_note};
use crate::follow::view::AnswerKind;
use crate::testing::call;

fn left(contained: bool, wait: InputWait, looks_secret: bool) -> Left {
    let mut left = Left::new(call(), "rsync -a u@h:x .", contained, Some(ApprovedBy::You));
    left.waits(wait, looks_secret, "u@h's password: ");
    left
}

#[test]
fn a_wait_in_the_sandbox_that_may_be_for_a_secret_asks_for_nothing() {
    let rows = [
        (false, InputWait::Hidden, false, Some(AnswerKind::Hidden)),
        (false, InputWait::Visible, true, Some(AnswerKind::Masked)),
        (false, InputWait::Visible, false, Some(AnswerKind::Visible)),
        (false, InputWait::None, false, None),
        (true, InputWait::Hidden, false, None),
        (true, InputWait::Visible, true, None),
        (true, InputWait::Visible, false, Some(AnswerKind::Visible)),
        (true, InputWait::None, false, None),
    ];
    for (contained, wait, looks_secret, kind) in rows {
        assert_eq!(left(contained, wait, looks_secret).kind(), kind, "{contained} {wait:?}");
    }
}

#[test]
fn only_a_visible_wait_can_look_secret() {
    let hidden = left(false, InputWait::Hidden, true);
    assert!(!hidden.looks_secret);
    assert!(hidden.secret_wait());
}

#[test]
fn the_end_line_says_password_only_outside_the_sandbox() {
    assert_eq!(
        left(false, InputWait::Hidden, false).end_line(true),
        "the hidden shell still runs: rsync -a u@h:x . (it waits for a password)"
    );
    assert_eq!(
        left(true, InputWait::Hidden, false).end_line(false),
        "the hidden shell still runs: rsync -a u@h:x . (it waits for input) \u{b7} ,shell answers or stops it"
    );
    assert_eq!(
        left(false, InputWait::None, false).end_line(false),
        "the hidden shell still runs: rsync -a u@h:x . \u{b7} ,shell answers or stops it"
    );
}

#[test]
fn the_label_names_who_let_it_run() {
    let mut shown = Left::new(call(), "make deploy", false, None);
    assert_eq!(shown.label(), "the hidden shell: make deploy");
    for (approved_by, words) in [
        (ApprovedBy::You, "you approved it"),
        (ApprovedBy::YourRule, "your rule allowed it"),
        (ApprovedBy::Reviewer, "the reviewer allowed it"),
        (ApprovedBy::FullMode, "full mode let it run"),
    ] {
        shown = Left::new(call(), "make deploy", false, Some(approved_by));
        assert_eq!(shown.label(), format!("the hidden shell: make deploy ({words})"));
    }
    let contained = Left::new(call(), "make", true, Some(ApprovedBy::Reviewer));
    assert_eq!(contained.label(), "the hidden shell: make (sandbox)");
    let unseen = Left::new(call(), " ", false, None);
    assert_eq!(unseen.label(), "the hidden shell: the command");
}

#[test]
fn the_command_and_the_prompt_never_carry_a_control_character() {
    let mut shown = Left::new(call(), "printf '\x1b]52;c;aGk=\x07'\nrm x", false, None);
    shown.waits(InputWait::Visible, false, "\x1b[2Jok?\r");
    shown.asking = shown.kind();
    let raw = shown.raw();
    assert!(!raw.chars().any(|c| c.is_control() && c != '\n'), "{raw:?}");
    assert_eq!(raw.lines().count(), 4, "{raw:?}");
}

#[test]
fn each_end_has_its_own_words() {
    let notes: Vec<String> = ShellCommandEnd::ALL
        .into_iter()
        .map(|how| ended_note("rsync -a x y", Some(1), how))
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
        ended_note("make", Some(0), ShellCommandEnd::Ended),
        "make ended in the hidden shell"
    );
}
