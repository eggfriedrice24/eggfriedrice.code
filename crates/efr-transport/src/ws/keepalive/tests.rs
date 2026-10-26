use pretty_assertions::assert_eq;

use super::Pings;

fn payload(number: u64) -> [u8; 8] {
    number.to_be_bytes()
}

#[test]
fn a_ping_waits_until_its_pong_comes() {
    let pings = Pings::default();
    assert_eq!(pings.outstanding(), 0);
    assert_eq!(pings.next(), 1);
    assert_eq!(pings.outstanding(), 1);
    pings.answer(&payload(1));
    assert_eq!(pings.outstanding(), 0);
}

#[test]
fn a_pong_answers_its_ping_and_every_older_one() {
    let pings = Pings::default();
    for _ in 0..3 {
        pings.next();
    }
    pings.answer(&payload(2));
    assert_eq!(pings.outstanding(), 1);
    // An older pong after a newer one changes nothing.
    pings.answer(&payload(1));
    assert_eq!(pings.outstanding(), 1);
}

#[test]
fn a_pong_that_names_no_ping_of_ours_answers_nothing() {
    let pings = Pings::default();
    pings.next();
    for foreign in [&b""[..], b"heartbeat", &payload(7)[..4], &payload(9)] {
        pings.answer(foreign);
        assert_eq!(pings.outstanding(), 1, "{foreign:?}");
    }
}
