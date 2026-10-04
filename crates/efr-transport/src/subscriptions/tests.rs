use efr_protocol::Seq;
use pretty_assertions::assert_eq;

use super::{Delivery, Offer, SUBSCRIBER_QUEUE_FRAMES, bounded, subscription};

fn item(seq: u64) -> Delivery<String> {
    Delivery::Item { seq: Seq::new(seq), item: format!("event {seq}") }
}

#[tokio::test]
async fn items_arrive_in_order_and_the_stream_ends_with_the_producer() {
    let (mut tx, mut rx) = subscription();
    assert_eq!(tx.offer(Seq::new(1), "event 1".to_owned()), Offer::Queued);
    assert_eq!(tx.offer(Seq::new(2), "event 2".to_owned()), Offer::Queued);
    drop(tx);
    assert_eq!(rx.recv().await, Some(item(1)));
    assert_eq!(rx.recv().await, Some(item(2)));
    assert_eq!(rx.recv().await, None);
    assert_eq!(rx.last_seq(), Some(Seq::new(2)));
}

#[tokio::test]
async fn the_queue_holds_sixty_four_items_and_the_next_offer_overflows() {
    let (mut tx, mut rx) = subscription();
    for seq in 1..=64 {
        assert_eq!(tx.offer(Seq::new(seq), format!("event {seq}")), Offer::Queued, "seq {seq}");
    }
    assert_eq!(SUBSCRIBER_QUEUE_FRAMES, 64);
    assert_eq!(tx.offer(Seq::new(65), "event 65".to_owned()), Offer::Overflowed);
    assert!(tx.is_closed());
    assert_eq!(tx.offer(Seq::new(66), "event 66".to_owned()), Offer::Closed);

    for seq in 1..=64 {
        assert_eq!(rx.recv().await, Some(item(seq)));
    }
    assert_eq!(rx.recv().await, Some(Delivery::Overflowed { last_seq: Seq::new(64) }));
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn an_overflow_with_nothing_delivered_reports_the_skipped_position() {
    let (mut tx, mut rx) = bounded::<String>(1);
    rx.skip_through(Seq::new(40));
    assert_eq!(tx.offer(Seq::new(39), "replayed already".to_owned()), Offer::Queued);
    assert_eq!(tx.offer(Seq::new(41), "event 41".to_owned()), Offer::Overflowed);
    assert_eq!(rx.recv().await, Some(Delivery::Overflowed { last_seq: Seq::new(40) }));
}

#[tokio::test]
async fn skip_through_drops_items_that_a_replay_already_sent() {
    let (mut tx, mut rx) = subscription();
    for seq in [5, 6, 7, 8] {
        assert_eq!(tx.offer(Seq::new(seq), format!("event {seq}")), Offer::Queued);
    }
    drop(tx);
    rx.skip_through(Seq::new(6));
    assert_eq!(rx.recv().await, Some(item(7)));
    assert_eq!(rx.recv().await, Some(item(8)));
    assert_eq!(rx.recv().await, None);
}

#[tokio::test]
async fn skip_through_never_moves_backwards() {
    let (_tx, mut rx) = subscription::<String>();
    rx.skip_through(Seq::new(9));
    rx.skip_through(Seq::new(3));
    assert_eq!(rx.last_seq(), Some(Seq::new(9)));
}

#[tokio::test]
async fn an_item_at_seq_zero_is_delivered() {
    // PTY output uses byte offsets, and the first chunk starts at offset 0.
    let (mut tx, mut rx) = subscription();
    assert_eq!(tx.offer(Seq::ZERO, "event 0".to_owned()), Offer::Queued);
    assert_eq!(rx.recv().await, Some(item(0)));
}

#[tokio::test]
async fn a_subscriber_that_went_away_closes_the_sender() {
    let (mut tx, rx) = subscription();
    drop(rx);
    assert!(tx.is_closed());
    assert_eq!(tx.offer(Seq::new(1), "event 1".to_owned()), Offer::Closed);
}
