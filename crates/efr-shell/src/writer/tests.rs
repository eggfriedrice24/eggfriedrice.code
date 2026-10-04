use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use bytes::Bytes;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt as _;
use tokio::sync::mpsc;

use super::{advance, write_loop};
use crate::reader::master;

#[test]
fn advance_drops_whole_and_partial_chunks() {
    let mut queue: VecDeque<Bytes> =
        [Bytes::from_static(b"abc"), Bytes::from_static(b"de"), Bytes::new()].into();
    advance(&mut queue, 4);
    assert_eq!(queue, [Bytes::from_static(b"e"), Bytes::new()]);
    advance(&mut queue, 1);
    assert!(queue.is_empty());
}

#[tokio::test]
async fn everything_sent_arrives_in_order_and_the_loop_ends_with_its_senders() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let master = master(OwnedFd::from(ours)).unwrap();
    let (writer, inbox) = mpsc::channel(4);
    let task = tokio::spawn(write_loop(master, inbox));
    for part in ["one ", "two ", "three"] {
        writer.send(Bytes::from(part)).await.unwrap();
    }
    drop(writer);
    task.await.unwrap();

    theirs.set_nonblocking(true).unwrap();
    let mut theirs = tokio::net::UnixStream::from_std(theirs).unwrap();
    let mut received = String::new();
    theirs.read_to_string(&mut received).await.unwrap();
    assert_eq!(received, "one two three");
}

#[tokio::test]
async fn senders_never_wait_on_a_peer_that_does_not_read() {
    let (ours, _theirs) = UnixStream::pair().unwrap();
    let master = master(OwnedFd::from(ours)).unwrap();
    let (writer, inbox) = mpsc::channel(1);
    tokio::spawn(write_loop(master, inbox));
    // Far more than a socket buffer holds; every send still completes.
    let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
    for _ in 0..64 {
        writer.send(chunk.clone()).await.unwrap();
    }
}
