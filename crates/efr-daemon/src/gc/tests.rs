use std::time::Duration;

use efr_shell::Phase;
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::gc::{Look, Seen, at_prompt, decide};

const IDLE: Duration = Duration::from_secs(3600);

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + seconds).unwrap()
}

fn idle_look(mark: u64) -> Look {
    Look { mark, at_prompt: true, in_use: false }
}

#[test]
fn a_new_shell_starts_its_quiet_time_at_the_first_look() {
    let (seen, close) = decide(None, idle_look(10), at(0), IDLE);

    assert_eq!(seen, Seen { mark: 10, since: at(0) });
    assert!(!close);
}

#[test]
fn a_quiet_shell_at_its_prompt_closes_after_the_idle_time() {
    let seen = Seen { mark: 10, since: at(0) };

    assert!(!decide(Some(seen), idle_look(10), at(3599), IDLE).1);
    assert!(decide(Some(seen), idle_look(10), at(3600), IDLE).1);
}

#[test]
fn output_or_input_restarts_the_quiet_time() {
    let seen = Seen { mark: 10, since: at(0) };

    let (next, close) = decide(Some(seen), idle_look(11), at(4000), IDLE);

    assert_eq!(next, Seen { mark: 11, since: at(4000) });
    assert!(!close);
}

#[test]
fn a_shell_in_use_never_closes_and_its_quiet_time_restarts() {
    let seen = Seen { mark: 10, since: at(0) };
    let look = Look { in_use: true, ..idle_look(10) };

    let (next, close) = decide(Some(seen), look, at(9000), IDLE);

    assert!(!close);
    assert_eq!(next.since, at(9000));
}

#[test]
fn a_shell_that_runs_a_command_never_closes() {
    let seen = Seen { mark: 10, since: at(0) };
    let look = Look { at_prompt: false, ..idle_look(10) };

    assert!(!decide(Some(seen), look, at(9000), IDLE).1);
}

#[test]
fn only_prompt_phases_count_as_waiting() {
    assert!(at_prompt(Phase::Ready));
    assert!(at_prompt(Phase::Prompting { continuation: false }));
    assert!(at_prompt(Phase::Unmarked));
    assert!(!at_prompt(Phase::Running));
    assert!(!at_prompt(Phase::Starting));
    assert!(!at_prompt(Phase::Continuation));
}
