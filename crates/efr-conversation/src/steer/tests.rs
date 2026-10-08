use pretty_assertions::assert_eq;

use super::Steering;

#[test]
fn steering_is_taken_oldest_first_and_once() {
    let steering = Steering::default();
    let shared = steering.clone();
    assert!(!steering.is_waiting());

    shared.reserve().expect("open").push("first".to_owned());
    shared.reserve().expect("open").push("second".to_owned());

    assert!(steering.is_waiting());
    assert_eq!(steering.take(), vec!["first".to_owned(), "second".to_owned()]);
    assert!(steering.take().is_empty());
    assert!(!steering.is_waiting());
}

#[tokio::test]
async fn a_closed_steering_gives_no_reservation() {
    let steering = Steering::default();
    assert!(steering.close_if_idle().await);
    assert!(steering.reserve().is_none());

    let other = Steering::default();
    other.reserve().expect("open").push("late".to_owned());
    other.close();
    assert!(other.reserve().is_none());
}

#[tokio::test]
async fn waiting_steering_keeps_the_steering_open() {
    let steering = Steering::default();
    steering.reserve().expect("open").push("more".to_owned());

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

    reservation.push("wait".to_owned());

    assert!(!closing.await.expect("the close ends"), "the pushed text keeps it open");
    assert_eq!(steering.take(), vec!["wait".to_owned()]);
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
