use efr_protocol::Seq;
use pretty_assertions::assert_eq;

use super::{Steer, Steering};

fn steer(seq: u64, text: &str) -> Steer {
    Steer { seq: Seq::new(seq), text: text.to_owned() }
}

fn texts(steers: &[Steer]) -> Vec<&str> {
    steers.iter().map(|steer| steer.text.as_str()).collect()
}

#[test]
fn steering_is_taken_oldest_first_and_once() {
    let steering = Steering::default();
    let shared = steering.clone();
    assert!(!steering.is_waiting());

    shared.reserve().expect("open").push(Seq::new(4), "first".to_owned());
    shared.reserve().expect("open").push(Seq::new(6), "second".to_owned());

    assert!(steering.is_waiting());
    assert_eq!(steering.take(), vec![steer(4, "first"), steer(6, "second")]);
    assert!(steering.take().is_empty());
    assert!(!steering.is_waiting());
}

#[tokio::test]
async fn a_closed_steering_gives_no_reservation() {
    let steering = Steering::default();
    assert!(steering.close_if_idle().await);
    assert!(steering.reserve().is_none());

    let other = Steering::default();
    other.reserve().expect("open").push(Seq::new(1), "late".to_owned());
    other.close();
    assert!(other.reserve().is_none());
}

#[tokio::test]
async fn waiting_steering_keeps_the_steering_open() {
    let steering = Steering::default();
    steering.reserve().expect("open").push(Seq::new(1), "more".to_owned());

    assert!(!steering.close_if_idle().await, "the turn makes one more call");
    assert!(steering.reserve().is_some(), "the steering is still open");
}

#[tokio::test]
async fn closing_waits_for_an_open_reservation() {
    let steering = Steering::default();
    let reservation = steering.reserve().expect("open");
    let closing = tokio::spawn({
        let steering = steering.clone();
        async move { steering.close_if_idle().await }
    });
    tokio::task::yield_now().await;
    assert!(!closing.is_finished(), "an open reservation holds the close");

    reservation.push(Seq::new(9), "wait".to_owned());

    assert!(!closing.await.expect("the close ends"), "the pushed text keeps it open");
    assert_eq!(steering.take(), vec![steer(9, "wait")]);
}

#[tokio::test]
async fn a_dropped_reservation_lets_the_steering_close() {
    let steering = Steering::default();
    let reservation = steering.reserve().expect("open");
    let closing = tokio::spawn({
        let steering = steering.clone();
        async move { steering.close_if_idle().await }
    });

    drop(reservation);

    assert!(closing.await.expect("the close ends"));
    assert!(steering.reserve().is_none());
}

#[test]
fn an_interrupt_takes_only_the_listed_steers_that_still_wait() {
    let steering = Steering::default();
    for (seq, text) in [(3, "a"), (5, "b"), (7, "c")] {
        steering.reserve().expect("open").push(Seq::new(seq), text.to_owned());
    }

    let taken = steering.take_unread(&[Seq::new(7), Seq::new(3), Seq::new(99)]);

    assert_eq!(texts(&taken), ["a", "c"], "in sequence order, unknown numbers skipped");
    assert_eq!(steering.take(), vec![steer(5, "b")], "a steer not listed still waits");
    assert!(steering.take_unread(&[Seq::new(5)]).is_empty(), "a steer that was read is not unread");
}

#[test]
fn steers_put_back_wait_in_their_order_again() {
    let steering = Steering::default();
    for (seq, text) in [(3, "a"), (5, "b"), (7, "c")] {
        steering.reserve().expect("open").push(Seq::new(seq), text.to_owned());
    }
    let taken = steering.take_unread(&[Seq::new(3), Seq::new(7)]);

    steering.put_back(taken);

    assert_eq!(texts(&steering.take()), ["a", "b", "c"]);
}
