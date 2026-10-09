use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{CallGap, millis};

fn at(seconds: i64) -> Timestamp {
    Timestamp::UNIX_EPOCH + SignedDuration::from_secs(seconds)
}

#[test]
fn each_call_measures_from_the_start_of_the_one_before() {
    let gap = CallGap::default();
    assert_eq!(gap.start(at(100)), None);
    assert_eq!(gap.start(at(130)), Some(Duration::from_secs(30)));
    assert_eq!(gap.start(at(130 + 3_600)), Some(Duration::from_secs(3_600)));
}

#[test]
fn a_clock_that_went_back_gives_no_negative_gap() {
    let gap = CallGap::default();
    gap.start(at(100));
    assert_eq!(gap.start(at(40)), Some(Duration::ZERO));
    assert_eq!(gap.start(at(45)), Some(Duration::from_secs(5)));
}

#[test]
fn the_field_is_whole_milliseconds() {
    assert_eq!(millis(Duration::from_micros(1_500_900)), 1_500);
    assert_eq!(millis(Duration::MAX), u64::MAX);
}
