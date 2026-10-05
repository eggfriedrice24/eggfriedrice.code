use std::os::unix::fs::PermissionsExt as _;

use efr_protocol::{CallId, ErrorBody, ErrorCode, Event, TurnId};
use pretty_assertions::assert_eq;

use crate::notices::{append, file_name, line};

fn turn() -> TurnId {
    TurnId::from_uuid(uuid::Uuid::from_u128(1))
}

#[test]
fn the_file_name_is_the_tty_without_dev_and_with_dashes() {
    assert_eq!(file_name("/dev/pts/3").as_deref(), Some("pts-3"));
    assert_eq!(file_name("/dev/tty1").as_deref(), Some("tty1"));
    assert_eq!(file_name("pts/12").as_deref(), Some("pts-12"));
}

#[test]
fn a_tty_that_could_escape_the_directory_gets_no_file() {
    assert_eq!(file_name(""), None);
    assert_eq!(file_name("/dev/"), None);
    assert_eq!(file_name(".."), None);
    assert_eq!(file_name("/dev/.."), None);
    assert_eq!(file_name("pts 3"), None);
    assert_eq!(file_name("pts\n3"), None);
}

#[test]
fn finished_and_failed_turns_and_waiting_approvals_get_a_line() {
    let completed = Event::TurnCompleted { turn_id: turn(), usage: None };
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Internal, "the provider stream broke"),
    };
    let approval = Event::ApprovalRequested {
        turn_id: turn(),
        call_id: CallId::from_uuid(uuid::Uuid::from_u128(2)),
        summary: "write /etc/hosts".to_owned(),
        diff_preview: None,
    };

    assert_eq!(
        line(&completed, Some("fix nginx")).as_deref(),
        Some("efr: turn finished: fix nginx")
    );
    assert_eq!(
        line(&failed, Some("fix nginx")).as_deref(),
        Some("efr: turn failed: fix nginx: the provider stream broke")
    );
    assert_eq!(
        line(&approval, None).as_deref(),
        Some("efr: approval waiting: a conversation: write /etc/hosts")
    );
}

#[test]
fn a_turn_that_failed_for_want_of_a_login_says_how_to_log_in() {
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Unauthorized, "no provider credentials are stored"),
    };

    assert_eq!(
        line(&failed, Some("fix nginx")).as_deref(),
        Some(
            "efr: turn failed: fix nginx: no provider credentials are stored; run efr login openai"
        )
    );
}

#[test]
fn the_login_hint_survives_a_long_title() {
    let failed = Event::TurnFailed {
        turn_id: turn(),
        error: ErrorBody::new(ErrorCode::Unauthorized, "the token was refused"),
    };
    let title = "x".repeat(400);

    let text = line(&failed, Some(&title)).unwrap();

    assert_eq!(text.chars().count(), 200);
    assert!(text.ends_with("...; run efr login openai"), "{text}");
}

#[test]
fn other_events_get_no_line() {
    assert_eq!(line(&Event::TurnInterrupted { turn_id: turn() }, Some("x")), None);
    assert_eq!(line(&Event::TurnCancelled { turn_id: turn() }, Some("x")), None);
}

#[test]
fn a_notice_stays_on_one_short_line() {
    let event = Event::TurnCompleted { turn_id: turn(), usage: None };
    let title = format!("two\nlines\x1b[31m{}", "x".repeat(400));

    let text = line(&event, Some(&title)).unwrap();

    assert!(!text.contains('\n') && !text.contains('\x1b'), "{text:?}");
    assert_eq!(text.chars().count(), 200);
    assert!(text.ends_with("..."));
}

#[test]
fn notices_append_to_a_private_file_per_tty() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("notices");

    let path = append(&dir, "/dev/pts/3", "efr: one").unwrap().unwrap();
    append(&dir, "/dev/pts/3", "efr: two").unwrap();
    append(&dir, "/dev/pts/4", "efr: other").unwrap();

    assert_eq!(path, dir.join("pts-3"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "efr: one\nefr: two\n");
    assert_eq!(std::fs::read_to_string(dir.join("pts-4")).unwrap(), "efr: other\n");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_tty_without_a_safe_name_writes_nothing() {
    let root = tempfile::tempdir().unwrap();

    assert_eq!(append(&root.path().join("notices"), "..", "efr: x").unwrap(), None);
    assert!(!root.path().join("notices").exists());
}
