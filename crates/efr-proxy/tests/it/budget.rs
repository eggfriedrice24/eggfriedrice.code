//! The upload budget (DP9): every client byte after the `CONNECT` head counts, the
//! ClientHello too, over every tunnel of the call. Each assert reads what the upstream
//! got, never the proxy's own count.

use efr_protocol::BlockReason;
use pretty_assertions::assert_eq;

use crate::kit::{Client, Upstream, World, list, record, rustls_hello, with_extension};

const BUDGET: u64 = 4096;

async fn world() -> (World, Upstream) {
    let world = World::new(list(&["allowed.test"], &[], &[]));
    let echo = Upstream::echo().await;
    world.table.add("allowed.test", &["127.0.0.1"]);
    world.connector.route("127.0.0.1:443", echo.addr);
    (world, echo)
}

fn budget() -> usize {
    usize::try_from(BUDGET).unwrap()
}

#[tokio::test]
async fn proxy_cuts_upload_budget() {
    let (world, echo) = world().await;
    let hello = rustls_hello("allowed.test", true);
    assert!(hello.len() < budget() / 2, "the test needs a small hello: {}", hello.len());

    // A hello with 10 KiB of padding counts against the budget.
    let (call, path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    client.send(&with_extension(&hello, 0x0015, 10 * 1024)).await;
    assert!(client.closed().await, "the tunnel is cut at the budget");
    echo.settled(1).await;
    assert_eq!(echo.total(), budget());
    assert_eq!(call.blocked().0[0].reason, BlockReason::UploadLimit);
    // Once the budget is spent, a new CONNECT gets 403 before any lookup or connect.
    let asked = world.table.asked().len();
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 403);
    assert_eq!(world.table.asked().len(), asked);
    assert_eq!(echo.got().len(), 1, "no new upstream connection");

    // Data after a small hello: the upstream gets at most the rest of the budget.
    let (call, path) = world.call(&[], BUDGET);
    let before = echo.total();
    let mut client = Client::connect(&path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
    client.send(&record(budget())).await;
    let _ = client.rest().await;
    echo.settled(2).await;
    assert_eq!(echo.total() - before, budget());
    assert_eq!(call.blocked().0[0].reason, BlockReason::UploadLimit);
}

#[tokio::test]
async fn proxy_budget_is_shared_by_the_tunnels_of_a_call() {
    let (world, echo) = world().await;
    let hello = rustls_hello("allowed.test", true);
    let (call, path) = world.call(&[], BUDGET);
    let first_data = record(1000);
    let mut first = Client::connect(&path).await;
    assert_eq!(first.connect_to("allowed.test:443").await, 200);
    first.send(&hello).await;
    first.send(&first_data).await;
    assert_eq!(
        first.exact(hello.len() + first_data.len()).await.len(),
        hello.len() + first_data.len()
    );
    let mut second = Client::connect(&path).await;
    assert_eq!(second.connect_to("allowed.test:443").await, 200);
    // One write: the cut may come in the hello already, and a later write would then
    // meet a closed socket.
    second.send(&[hello.clone(), record(budget())].concat()).await;
    let _ = second.rest().await;
    // The first tunnel is cut at its next byte.
    first.send(b"x").await;
    assert!(first.closed().await);
    echo.settled(2).await;
    assert_eq!(echo.total(), budget(), "both tunnels together sent the budget");
    let mut third = Client::connect(&path).await;
    assert_eq!(third.connect_to("allowed.test:443").await, 403);
    assert_eq!(echo.got().len(), 2, "the third tunnel never reached the upstream");
    let (records, _) = call.blocked();
    assert_eq!(records.len(), 1);
    assert_eq!((records[0].reason, records[0].count), (BlockReason::UploadLimit, 3));

    // A second call has a budget of its own.
    let (other, other_path) = world.call(&[], BUDGET);
    let mut client = Client::connect(&other_path).await;
    assert_eq!(client.connect_to("allowed.test:443").await, 200);
    client.send(&hello).await;
    assert_eq!(client.exact(hello.len()).await, hello);
    assert_eq!(other.blocked(), (Vec::new(), 0));
}
