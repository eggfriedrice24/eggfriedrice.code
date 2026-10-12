use efr_protocol::{CallId, Mode, NoticeKind, Seq, TurnId};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{
    NOTICE_BYTES_PER_TURN, NOTICES_PER_TURN, WakeWindow, more_notices, take_notices, wake_mode,
};
use crate::JobNoticeInput;
use crate::testing::{conversation_id, job_info};

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_second(1_791_000_000).unwrap() + SignedDuration::from_mins(minutes)
}

#[test]
fn wake_limit_six_per_hour() {
    // (minute of a notice, whether it may start a turn); each allowed one starts one.
    let table = [
        (0, true),
        (5, true),
        (10, true),
        (20, true),
        (30, true),
        (40, true),
        (50, false),
        (59, false),
        (60, true),
        (61, false),
        (65, true),
    ];
    let mut window = WakeWindow::default();
    for (minute, allowed) in table {
        let now = at(minute);
        assert_eq!(window.allows(now, 6), allowed, "minute {minute}");
        if allowed {
            window.record(now);
        }
    }
}

#[test]
fn a_seeded_window_counts_the_starts_before_a_restart() {
    let mut window = WakeWindow::new([at(30), at(10)]);
    assert!(!window.allows(at(40), 2));
    assert!(window.allows(at(71), 2), "the start at minute 10 left the hour");
}

#[test]
fn a_limit_of_zero_never_wakes() {
    assert!(!WakeWindow::default().allows(at(0), 0));
}

#[rstest]
#[case(Mode::Auto, Mode::Auto, Mode::Auto, Mode::Auto)]
#[case(Mode::Auto, Mode::Manual, Mode::Auto, Mode::Manual)]
#[case(Mode::Cautious, Mode::Auto, Mode::Auto, Mode::Cautious)]
#[case(Mode::Auto, Mode::Auto, Mode::Cautious, Mode::Cautious)]
fn wake_turn_mode_is_the_lowest(
    #[case] start: Mode,
    #[case] newest: Mode,
    #[case] config: Mode,
    #[case] wake: Mode,
) {
    assert_eq!(wake_mode(start, newest, config), wake);
}

fn notice(number: u32, bytes: usize) -> JobNoticeInput {
    let turn: TurnId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let call: CallId = "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap();
    let job = job_info(conversation_id(), turn, call, number, "make");
    JobNoticeInput::new(
        job,
        NoticeKind::Exited,
        Seq::new(u64::from(number)),
        "x".repeat(bytes),
        true,
    )
}

#[test]
fn a_turn_takes_at_most_ten_notices_and_32_kib() {
    let mut waiting: Vec<JobNoticeInput> = (1..=12).map(|n| notice(n, 100)).collect();
    let taken = take_notices(&mut waiting);
    assert_eq!(taken.len(), NOTICES_PER_TURN);
    assert_eq!(waiting.len(), 2);
    assert_eq!(more_notices(waiting.len()), "2 more job notices; call jobs to read them");

    let mut big: Vec<JobNoticeInput> = (1..=5).map(|n| notice(n, 9 * 1024)).collect();
    let taken = take_notices(&mut big);
    assert_eq!(taken.len(), 3, "a fourth would pass {NOTICE_BYTES_PER_TURN} bytes");
    assert_eq!(big.len(), 2);
    assert_eq!(more_notices(1), "1 more job notice; call jobs to read it");
}
