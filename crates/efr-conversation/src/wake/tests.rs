use efr_protocol::{CallId, Mode, NoticeKind, Origin, Seq, TurnId};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{
    MAX_WAKE_DEPTH, NOTICE_BYTES_PER_TURN, NOTICES_PER_TURN, WakeWindow, more_notices, next_depth,
    take_notices, wake_mode,
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

/// A wake turn is one deeper than the deepest job turn of its notices, so a batch of a
/// prompt's job and a wake turn's job never makes the chain shorter.
#[rstest]
#[case(&[0], 1)]
#[case(&[1], MAX_WAKE_DEPTH)]
#[case(&[0, 1], MAX_WAKE_DEPTH)]
#[case(&[1, 0], MAX_WAKE_DEPTH)]
#[case(&[u8::MAX], u8::MAX)]
fn a_wake_turn_is_one_deeper_than_its_deepest_job(#[case] depths: &[u8], #[case] want: u8) {
    assert_eq!(next_depth(depths.iter().copied()), want);
}

#[rstest]
#[case(Mode::Auto, None, Mode::Auto)]
#[case(Mode::Cautious, None, Mode::Cautious)]
#[case(Mode::Auto, Some(Mode::Manual), Mode::Manual)]
#[case(Mode::Cautious, Some(Mode::Auto), Mode::Auto)]
#[case(Mode::Manual, Some(Mode::Cautious), Mode::Cautious)]
fn wake_turn_mode_is_the_newest_prompts(
    #[case] start: Mode,
    #[case] newest: Option<Mode>,
    #[case] wake: Mode,
) {
    assert_eq!(wake_mode(start, newest, Origin::Shell), wake);
}

/// The review finding: the job of a phone turn in cautious woke in the auto of a later
/// terminal prompt, so a phone turn ran in auto that the phone never named.
#[rstest]
#[case(Mode::Cautious, None, Mode::Cautious)]
#[case(Mode::Cautious, Some(Mode::Auto), Mode::Cautious)]
#[case(Mode::Cautious, Some(Mode::Full), Mode::Cautious)]
#[case(Mode::Auto, Some(Mode::Manual), Mode::Manual)]
#[case(Mode::Auto, Some(Mode::Auto), Mode::Auto)]
fn a_phone_wake_turn_is_never_looser_than_the_turn_of_its_job(
    #[case] start: Mode,
    #[case] newest: Option<Mode>,
    #[case] wake: Mode,
) {
    assert_eq!(wake_mode(start, newest, Origin::Phone), wake);
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
