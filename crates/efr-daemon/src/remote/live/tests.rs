use std::sync::Arc;

use efr_protocol::{DeviceId, ScopeName};
use efr_store::Store;
use efr_store::devices::{DevicePeer, DevicePublicKey, NewDevice};
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::{LiveDevices, follow};

fn device(n: u128) -> DeviceId {
    DeviceId::from_uuid(uuid::Uuid::from_u128(n))
}

#[test]
fn a_close_cancels_every_connection_of_the_device_and_only_those() {
    let live = LiveDevices::default();
    let (a1, a2, b) =
        (CancellationToken::new(), CancellationToken::new(), CancellationToken::new());
    live.add(device(1), 1, a1.clone());
    live.add(device(1), 2, a2.clone());
    live.add(device(2), 3, b.clone());

    assert_eq!(live.close(device(1)), 2);
    assert!(a1.is_cancelled() && a2.is_cancelled());
    assert!(!b.is_cancelled());
    assert_eq!(live.count(device(1)), 0);

    let twice = CancellationToken::new();
    live.add(device(3), 4, twice.clone());
    live.add(device(3), 4, twice.clone());
    live.remove(device(3), 4);
    assert_eq!(live.count(device(3)), 1, "a refused proof takes out its own entry only");
    live.remove(device(3), 4);
    live.forget(3);
    assert_eq!(live.devices(), Vec::<DeviceId>::new());
    assert_eq!(live.close(device(2)), 0, "a forgotten connection is not closed");
    assert!(!b.is_cancelled());
}

#[tokio::test]
async fn a_revoke_on_the_feed_closes_the_device() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let live = Arc::new(LiveDevices::default());
    let close = CancellationToken::new();
    live.add(device(1), 1, close.clone());
    let (feed, receiver) = broadcast::channel(4);
    let stop = CancellationToken::new();
    let task =
        tokio::spawn(follow(Arc::clone(&live), store.readers().clone(), receiver, stop.clone()));

    feed.send(device(1)).unwrap();
    Wait::new("the revoked device's connection to close")
        .until(|| close.is_cancelled())
        .await
        .unwrap();

    stop.cancel();
    task.await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn a_follower_that_fell_behind_checks_every_live_device_again() {
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.shared()).await.unwrap();
    let mut key = vec![4_u8];
    key.extend([1; 64]);
    let peer = DevicePeer {
        node_stable_id: "nPHONE".to_owned(),
        user_id: 42,
        tailnet_stable_id: "T1".to_owned(),
    };
    let kept = NewDevice::new(
        device(1),
        "phone",
        DevicePublicKey::from_x963(&key).unwrap(),
        "SHA256:abc",
        peer,
        &[ScopeName::Read],
    )
    .unwrap();
    store.writer().device_insert(kept).await.unwrap();

    let live = Arc::new(LiveDevices::default());
    let (stays, unknown) = (CancellationToken::new(), CancellationToken::new());
    live.add(device(1), 1, stays.clone());
    live.add(device(2), 2, unknown.clone());
    let (feed, receiver) = broadcast::channel(1);
    // NOTE: two sends on a room of one: the follower misses the first.
    feed.send(device(9)).unwrap();
    feed.send(device(9)).unwrap();
    let stop = CancellationToken::new();
    let task =
        tokio::spawn(follow(Arc::clone(&live), store.readers().clone(), receiver, stop.clone()));

    Wait::new("the unknown device's connection to close")
        .until(|| unknown.is_cancelled())
        .await
        .unwrap();
    assert!(!stays.is_cancelled(), "an active device stays");

    stop.cancel();
    task.await.unwrap();
    store.close().await;
}
