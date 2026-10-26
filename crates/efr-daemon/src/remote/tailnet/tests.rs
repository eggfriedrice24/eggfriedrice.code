use std::net::Ipv4Addr;
use std::time::Duration;

use efr_http::{LocalApi, TailscaleStatus, TailscaleVersion};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_support::{TestClock, WAIT_LIMIT, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{Down, Tailnet, View, follow, publish, view_of};
use efr_test_support::tailscaled::{self as fake, FakeTailscaled};

/// The status of `value`, with the version that tailscaled's header gives.
fn parsed(value: Value, version: Option<&str>) -> TailscaleStatus {
    let mut status: TailscaleStatus = serde_json::from_value(value).unwrap();
    status.version = version.and_then(TailscaleVersion::parse);
    status
}

fn running() -> Value {
    fake::status(fake::SELF_IP, fake::TAILNET)
}

fn up() -> Tailnet {
    Tailnet {
        ipv4: Ipv4Addr::new(100, 101, 102, 103),
        own_ips: vec![fake::SELF_IP.parse().unwrap(), "fd7a:115c:a1e0::1234:5678".parse().unwrap()],
        self_node: fake::SELF_NODE.to_owned(),
        self_user: Some(fake::USER),
        tailnet: fake::TAILNET.to_owned(),
        fqdn: Some("desk.example.ts.net".to_owned()),
    }
}

#[test]
fn a_running_node_with_tun_and_a_new_tailscaled_is_up() {
    assert_eq!(view_of(&parsed(running(), Some(fake::VERSION))), View::Up(up()));
}

#[test]
fn each_state_that_cannot_hold_a_listener_is_down_with_its_reason() {
    let mut stopped = running();
    stopped["BackendState"] = json!("Stopped");
    let mut userspace = running();
    userspace["TUN"] = json!(false);
    let mut no_ipv4 = running();
    no_ipv4["TailscaleIPs"] = json!(["fd7a:115c:a1e0::1234:5678", "192.168.1.5"]);
    let mut no_tailnet = running();
    no_tailnet["CurrentTailnet"]["StableID"] = json!("");
    let mut no_self = running();
    no_self["Self"] = Value::Null;

    let cases = [
        (stopped, Some(fake::VERSION), Down::NotRunning),
        (userspace, Some(fake::VERSION), Down::Userspace),
        (running(), Some("1.98.0"), Down::OldVersion),
        (running(), None, Down::OldVersion),
        (no_ipv4, Some(fake::VERSION), Down::NoAddress),
        (no_tailnet, Some(fake::VERSION), Down::NoIdentity),
        (no_self, Some(fake::VERSION), Down::NoIdentity),
    ];
    for (status, version, down) in cases {
        assert_eq!(view_of(&parsed(status, version)), View::Down(down), "{down:?}");
    }
}

#[test]
fn a_tagged_node_has_no_owner_so_no_peer_can_be_its_user() {
    let mut tagged = running();
    tagged["Self"]["Tags"] = json!(["tag:server"]);
    let View::Up(tailnet) = view_of(&parsed(tagged, Some(fake::VERSION))) else {
        panic!("a tagged node can still listen");
    };
    assert_eq!(tailnet.self_user, None);
}

/// Waits until `views` shows `want`.
async fn wait_for(views: &mut watch::Receiver<View>, want: &View) {
    let waited = SystemClock.timeout(WAIT_LIMIT, views.wait_for(|view| view == want));
    waited.await.expect("the view did not come").unwrap();
}

#[tokio::test]
async fn the_follower_reads_the_watch_then_asks_the_status_when_it_ends() {
    let fake = FakeTailscaled::start().unwrap();
    let clock = TestClock::new();
    let api = LocalApi::new(fake.socket(), clock.shared());
    let (views, mut view) = watch::channel(View::Unknown);
    let stop = CancellationToken::new();
    let task = tokio::spawn(follow(api, clock.shared(), views, stop.clone()));

    wait_for(&mut view, &View::Up(up())).await;
    assert!(fake.seen().iter().any(|target| target == "/localapi/v0/watch-ipn-bus?mask=16386"));

    // A new address arrives as a change of this node; the follower asks the status.
    fake.set_status(Some(fake::status("100.101.102.104", fake::TAILNET)));
    fake.push(&json!({ "SelfChange": { "StableID": fake::SELF_NODE, "Addresses": ["100.101.102.104/32"] } }));
    let mut moved = up();
    moved.ipv4 = Ipv4Addr::new(100, 101, 102, 104);
    moved.own_ips[0] = "100.101.102.104".parse().unwrap();
    wait_for(&mut view, &View::Up(moved.clone())).await;

    // A reader that fell behind is dropped; the status still answers, so it stays up.
    fake.push(&json!({ "ErrMessage": "IPN bus consumer fell behind" }));
    fake.end_watches();
    Wait::new("the pause before the next watch")
        .until(|| clock.pending_sleeps() == 1)
        .await
        .unwrap();
    assert_eq!(*view.borrow(), View::Up(moved));

    // Then tailscaled goes away: the watch ends and the status fails.
    fake.set_status(None);
    clock.advance(Duration::from_secs(1));
    wait_for(&mut view, &View::Down(Down::Unreachable)).await;

    stop.cancel();
    task.await.unwrap();
}

#[test]
fn a_view_that_alters_nothing_is_not_sent_again() {
    let (views, mut view) = watch::channel(View::Unknown);
    publish(&views, View::Up(up()));
    assert!(view.has_changed().unwrap());
    view.mark_unchanged();
    publish(&views, View::Up(up()));
    assert!(!view.has_changed().unwrap(), "the same node rebinds nothing");
    publish(&views, View::Down(Down::NotRunning));
    assert!(view.has_changed().unwrap());
}
