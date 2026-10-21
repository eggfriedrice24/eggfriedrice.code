//! The lines of a command left running in the hidden shell.

use std::time::Duration;

use efr_protocol::{ApprovedBy, CallId, Event, InputWait, ShellCommandEnd};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::notices::{Left, LeftCommands, shell_line};

/// The call of the command left running.
fn call() -> CallId {
    CallId::from_uuid(uuid::Uuid::from_u128(7))
}

/// When the command left running started.
fn since() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}

/// The `shell_command_detached` of an rsync left running with `input`.
fn detached(
    input: InputWait,
    contained: bool,
    approved_by: Option<ApprovedBy>,
    prompt: &str,
) -> Event {
    Event::ShellCommandDetached {
        call_id: call(),
        command: "rsync -a -e 'ssh -o PreferredAuthentications=password' u@h:src/ dst/".to_owned(),
        since: since(),
        contained,
        input,
        looks_secret: false,
        prompt: prompt.to_owned(),
        approved_by,
    }
}

/// The `shell_command_ended` of the command, ended `how`.
fn ended(how: ShellCommandEnd) -> Event {
    Event::ShellCommandEnded { call_id: call(), exit_code: None, how }
}

/// [`shell_line`] for a conversation titled `sync`, `after` the command's start, with
/// the limit of the default settings, an hour.
fn text(event: &Event, left: Option<&Left>, after: Duration) -> Option<String> {
    let at = since().checked_add(after).unwrap();
    shell_line(event, Some("sync"), left, at, Duration::from_secs(3600))
}

#[test]
fn a_command_left_running_gets_a_line_that_names_it() {
    let event = detached(InputWait::None, false, Some(ApprovedBy::You), "");
    let line = text(&event, None, Duration::ZERO).unwrap();
    // The line is cut to its first 57 characters, so the hint fits.
    assert_eq!(
        line,
        "efr: sync: the hidden shell still runs rsync -a -e 'ssh -o \
         PreferredAuthentications=password' u@...; ,shell shows it and stops it"
    );
    let long = "x".repeat(400);
    let line = shell_line(&event, Some(&long), None, since(), Duration::from_secs(3600)).unwrap();
    assert!(line.ends_with("...; ,shell shows it and stops it"), "{line}");
    assert!(line.chars().count() <= 200, "{line}");
}

#[test]
fn a_kept_password_wait_names_the_row_the_approver_and_where_to_type() {
    let event = detached(InputWait::Hidden, false, Some(ApprovedBy::You), "u@h's password: ");
    let line = text(&event, None, Duration::ZERO).unwrap();
    assert!(
        line.contains("waits for a password (asked by \"u@h's password:\"; you approved it)"),
        "{line}"
    );
    assert!(
        line.ends_with(". Type the password only after ,shell; never at this prompt."),
        "{line}"
    );
    // A long title is cut, never the row, the approver or where to type.
    let long = "t".repeat(400);
    let line = shell_line(&event, Some(&long), None, since(), Duration::from_secs(3600)).unwrap();
    assert!(
        line.ends_with(
            "... and waits for a password (asked by \"u@h's password:\"; you approved it). \
             Type the password only after ,shell; never at this prompt."
        ),
        "{line}"
    );
    assert!(line.chars().count() <= 300, "{line}");

    // A later wait names the command of the detach and its approver.
    let mut known = LeftCommands::default();
    known.see(&detached(InputWait::None, false, Some(ApprovedBy::YourRule), ""));
    let changed = Event::ShellCommandInputChanged {
        call_id: call(),
        input: InputWait::Visible,
        contained: false,
        looks_secret: false,
        prompt: "Continue? [Y/n] ".to_owned(),
    };
    let left = known.see(&changed);
    let line = text(&changed, left.as_ref(), Duration::ZERO).unwrap();
    assert!(line.contains("still runs rsync -a"), "{line}");
    assert!(
        line.contains("waits for input (asked by \"Continue? [Y/n]\"; your rule allowed it)"),
        "{line}"
    );
    assert!(line.ends_with(". Answer it only after ,shell; never at this prompt."), "{line}");
}

#[test]
fn a_password_wait_that_efr_does_not_keep_gets_no_line() {
    // In the sandbox, and for a call that no person approved, efr stops it at once, and
    // the stop gets a line.
    for (contained, approved_by) in [
        (true, Some(ApprovedBy::You)),
        (false, Some(ApprovedBy::Reviewer)),
        (false, Some(ApprovedBy::FullMode)),
        (false, None),
    ] {
        let event = detached(InputWait::Hidden, contained, approved_by, "pw: ");
        assert_eq!(text(&event, None, Duration::ZERO), None, "{contained} {approved_by:?}");
    }
    // The end of a wait is no news.
    let quiet = Event::ShellCommandInputChanged {
        call_id: call(),
        input: InputWait::None,
        contained: false,
        looks_secret: false,
        prompt: String::new(),
    };
    assert_eq!(text(&quiet, None, Duration::ZERO), None);
    // A plain question of a reviewed call stays: it never asks for a secret.
    let question = detached(InputWait::Visible, false, Some(ApprovedBy::Reviewer), "ok? [y/N] ");
    let line = text(&question, None, Duration::ZERO).unwrap();
    assert!(line.contains("(asked by \"ok? [y/N]\"; the reviewer allowed it)"), "{line}");
}

#[test]
fn a_stop_by_efr_says_why() {
    let mut known = LeftCommands::default();
    known.see(&detached(InputWait::Visible, false, Some(ApprovedBy::You), "name: "));
    let left = known.see(&ended(ShellCommandEnd::Limit));
    assert!(left.is_some(), "the end hands over what was known");
    assert_eq!(known.see(&ended(ShellCommandEnd::Limit)), None, "and forgets it");
    let hour = Duration::from_secs(3600);
    assert_eq!(
        text(&ended(ShellCommandEnd::Limit), left.as_ref(), hour).as_deref(),
        Some(
            "efr: sync: efr stopped rsync -a -e 'ssh -o PreferredAuthentications=password' \
             u@... in the hidden shell: it waited for input for 60 minutes"
        )
    );

    let mut known = LeftCommands::default();
    known.see(&detached(InputWait::Hidden, false, Some(ApprovedBy::Reviewer), "pw: "));
    let left = known.see(&ended(ShellCommandEnd::Limit));
    let line = text(&ended(ShellCommandEnd::Limit), left.as_ref(), Duration::from_secs(2)).unwrap();
    assert!(
        line.ends_with(" in the hidden shell: it asked for a password while nobody followed"),
        "{line}"
    );

    let mut known = LeftCommands::default();
    known.see(&detached(InputWait::Hidden, true, Some(ApprovedBy::You), "pw: "));
    let left = known.see(&ended(ShellCommandEnd::Limit));
    let line = text(&ended(ShellCommandEnd::Limit), left.as_ref(), hour).unwrap();
    assert!(
        line.ends_with(": it asked for a secret in the sandbox, where efr types none"),
        "{line}"
    );

    // The user's own stop, and a command that ended by itself, need no line.
    for how in [ShellCommandEnd::Stopped, ShellCommandEnd::Closed, ShellCommandEnd::Ended] {
        assert_eq!(text(&ended(how), left.as_ref(), hour), None, "{how:?}");
    }
}

#[test]
fn a_line_of_a_command_left_running_is_flat() {
    let event = Event::ShellCommandDetached {
        call_id: call(),
        command: "printf '\x1b]52;c;aGk=\x07'\nrm -rf x".to_owned(),
        since: since(),
        contained: false,
        input: InputWait::Visible,
        looks_secret: false,
        prompt: "say\u{202e}yes: \x1b[2J".to_owned(),
        approved_by: Some(ApprovedBy::You),
    };
    let line = text(&event, None, Duration::ZERO).unwrap();
    assert!(!line.chars().any(char::is_control), "{line:?}");
    assert!(!line.contains('\u{202e}'), "{line:?}");
    assert!(line.contains("say\u{fffd}yes"), "{line:?}");
}
