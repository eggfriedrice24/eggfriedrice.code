use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use efr_protocol::{Cursor, ScreenSnapshot, Seq, Size};
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{COMMAND_CAPACITY, ScreenActor, ScreenEvent, ScreenEvents, ScreenHandle};
use crate::fake::{DA1_REPLY, FakeScreen};
use crate::snapshot::{ScreenCapture, row_text};
use crate::{
    PromptKind, Screen, ScreenError, ScreenSink, SemanticPromptEvent, ShellMark, ShellMarkKind,
};

fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

fn spawn_fake(cols: u16, rows: u16) -> (ScreenHandle, ScreenEvents) {
    ScreenActor::spawn("screen-test", FakeScreen::factory(size(cols, rows)), size(cols, rows))
        .unwrap()
}

fn bytes(data: &'static [u8]) -> Bytes {
    Bytes::from_static(data)
}

/// Every event up to the answer of `request_snapshot(id)`, and that answer.
async fn until_snapshot(
    handle: &ScreenHandle,
    events: &mut ScreenEvents,
    id: u64,
) -> (Vec<ScreenEvent>, ScreenCapture) {
    handle.request_snapshot(id, 0).await.unwrap();
    let mut seen = Vec::new();
    loop {
        match events.recv().await.expect("the screen stopped before the snapshot") {
            ScreenEvent::Snapshot { id: got, capture } if got == id => return (seen, capture),
            event => seen.push(event),
        }
    }
}

fn rows_text(capture: &ScreenCapture) -> Vec<String> {
    capture.snapshot.rows.iter().map(row_text).collect()
}

fn prompt_start() -> ShellMarkKind {
    ShellMarkKind::SemanticPrompt(SemanticPromptEvent::PromptStart {
        kind: PromptKind::Initial,
        aid: None,
        click: None,
        fresh_line: true,
    })
}

/// Polls `future` once with the current task's waker.
async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn the_factory_runs_on_the_named_thread() {
    let (tell, told) = std_mpsc::channel();
    let factory = move || {
        tell.send(std::thread::current().name().map(str::to_owned)).unwrap();
        FakeScreen::new(size(4, 1))
    };
    let (handle, _events) = ScreenActor::spawn("screen-0a1b", factory, size(4, 1)).unwrap();
    assert_eq!(told.recv().unwrap().as_deref(), Some("screen-0a1b"));
    assert_eq!(handle.name(), "screen-0a1b");
}

#[tokio::test]
async fn a_screen_that_is_not_send_renders_what_it_is_fed() {
    // FakeScreen holds an Rc, so this compiles only because the actor never moves the
    // screen between threads.
    let (handle, _events) = spawn_fake(10, 2);
    handle.feed(bytes(b"hi\r\nyou"), Seq::ZERO).await.unwrap();
    let capture = handle.snapshot(0).await.unwrap();
    assert_eq!(rows_text(&capture), vec!["hi", "you"]);
    assert_eq!(capture.snapshot.cursor, Cursor { row: 1, col: 3, hidden: false });
}

#[tokio::test]
async fn replies_leave_after_the_feed() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"\x1b[c"), Seq::ZERO).await.unwrap();
    assert_eq!(events.recv().await, Some(ScreenEvent::PtyReply(Bytes::from_static(DA1_REPLY))));
}

#[tokio::test]
async fn the_replies_of_one_feed_arrive_as_one_event() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"ab\x1b[c\x1b[6n"), Seq::ZERO).await.unwrap();
    let (seen, _) = until_snapshot(&handle, &mut events, 1).await;
    let expected = [DA1_REPLY, b"\x1b[1;3R"].concat();
    assert_eq!(seen, vec![ScreenEvent::PtyReply(Bytes::from(expected))]);
}

#[tokio::test]
async fn marks_come_first_then_notices_then_replies() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"\x1b[c\x07\x1b]133;A\x07\x1b]2;build\x07"), Seq::ZERO).await.unwrap();
    let (seen, _) = until_snapshot(&handle, &mut events, 1).await;
    assert_eq!(
        seen,
        vec![
            ScreenEvent::ShellMark(ShellMark {
                start: Seq::new(4),
                end: Seq::new(12),
                kind: prompt_start(),
            }),
            ScreenEvent::Bell,
            ScreenEvent::TitleChanged("build".to_owned()),
            ScreenEvent::PtyReply(Bytes::from_static(DA1_REPLY)),
        ]
    );
}

#[tokio::test]
async fn marks_carry_recording_offsets_from_the_feed() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"$ \x1b]133;A\x07"), Seq::new(1000)).await.unwrap();
    let expected = ShellMark { start: Seq::new(1002), end: Seq::new(1010), kind: prompt_start() };
    assert_eq!(events.recv().await, Some(ScreenEvent::ShellMark(expected)));
}

#[tokio::test]
async fn a_mark_split_across_feeds_is_reported_once() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"\x1b]133;"), Seq::new(0)).await.unwrap();
    handle.feed(bytes(b"D;0\x07"), Seq::new(6)).await.unwrap();
    let (seen, _) = until_snapshot(&handle, &mut events, 1).await;
    let kind = ShellMarkKind::SemanticPrompt(SemanticPromptEvent::CommandEnd {
        exit_code: Some(0),
        error: None,
        aid: None,
    });
    assert_eq!(
        seen,
        vec![ScreenEvent::ShellMark(ShellMark { start: Seq::ZERO, end: Seq::new(10), kind })]
    );
}

#[tokio::test]
async fn a_snapshot_reports_the_offset_after_the_last_feed() {
    let (handle, _events) = spawn_fake(10, 2);
    assert_eq!(handle.snapshot(0).await.unwrap().at, Seq::ZERO);
    handle.feed(bytes(b"hello"), Seq::new(10)).await.unwrap();
    assert_eq!(handle.snapshot(0).await.unwrap().at, Seq::new(15));
}

#[tokio::test]
async fn the_screen_starts_at_the_spawn_size() {
    let (handle, _events) =
        ScreenActor::spawn("screen-test", FakeScreen::factory(size(4, 2)), size(12, 3)).unwrap();
    assert_eq!(handle.snapshot(0).await.unwrap().snapshot.size, size(12, 3));
}

#[tokio::test]
async fn resize_changes_the_size() {
    let (handle, _events) = spawn_fake(10, 2);
    handle.feed(bytes(b"abc"), Seq::ZERO).await.unwrap();
    handle.resize(size(20, 4)).await.unwrap();
    let capture = handle.snapshot(0).await.unwrap();
    assert_eq!(capture.snapshot.size, size(20, 4));
    assert_eq!(rows_text(&capture), vec!["abc", "", "", ""]);
}

#[tokio::test]
async fn a_requested_snapshot_follows_the_events_before_it() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"\x07"), Seq::ZERO).await.unwrap();
    handle.request_snapshot(7, 0).await.unwrap();
    handle.feed(bytes(b"\x07"), Seq::new(1)).await.unwrap();
    assert_eq!(events.recv().await, Some(ScreenEvent::Bell));
    match events.recv().await {
        Some(ScreenEvent::Snapshot { id: 7, capture }) => assert_eq!(capture.at, Seq::new(1)),
        other => panic!("expected the snapshot, got {other:?}"),
    }
    assert_eq!(events.recv().await, Some(ScreenEvent::Bell));
}

#[tokio::test]
async fn snapshots_are_normalized() {
    let (handle, _events) = spawn_fake(10, 2);
    handle.feed(bytes(b"1\r\n2\r\n3\r\n4"), Seq::ZERO).await.unwrap();
    let capture = handle.snapshot(1).await.unwrap();
    assert_eq!(capture.snapshot.rows[0].cells.len(), 1);
    assert_eq!(capture.snapshot.scrollback.iter().map(row_text).collect::<Vec<_>>(), vec!["2"]);
}

#[tokio::test]
async fn shutdown_ends_the_event_stream_after_the_events_before_it() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed(bytes(b"\x07"), Seq::ZERO).await.unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(events.recv().await, Some(ScreenEvent::Bell));
    assert_eq!(events.recv().await, None);
    let err = handle.feed(bytes(b"x"), Seq::new(1)).await.unwrap_err();
    assert!(matches!(err, ScreenError::Closed { name } if name == "screen-test"));
    assert!(matches!(handle.snapshot(0).await, Err(ScreenError::Closed { .. })));
}

#[tokio::test]
async fn dropping_every_handle_stops_the_actor() {
    let (handle, mut events) = spawn_fake(10, 2);
    let clone = handle.clone();
    drop(handle);
    clone.feed(bytes(b"\x07"), Seq::ZERO).await.unwrap();
    drop(clone);
    assert_eq!(events.recv().await, Some(ScreenEvent::Bell));
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn a_dropped_event_stream_does_not_stop_the_screen() {
    let (handle, events) = spawn_fake(10, 2);
    drop(events);
    // Far more replies than the event queue holds: the actor must discard them.
    for index in 0..(4 * COMMAND_CAPACITY as u64) {
        handle.feed(bytes(b"\x1b[c"), Seq::new(index * 3)).await.unwrap();
    }
    let capture = handle.snapshot(0).await.unwrap();
    assert_eq!(capture.at, Seq::new(4 * COMMAND_CAPACITY as u64 * 3));
}

#[test]
fn the_blocking_methods_work_from_a_plain_thread() {
    let (handle, mut events) = spawn_fake(10, 2);
    handle.feed_blocking(bytes(b"ok\x1b[c"), Seq::ZERO).unwrap();
    handle.resize_blocking(size(8, 3)).unwrap();
    handle.request_snapshot_blocking(3, 0).unwrap();
    assert_eq!(events.blocking_recv(), Some(ScreenEvent::PtyReply(Bytes::from_static(DA1_REPLY))));
    assert!(matches!(events.blocking_recv(), Some(ScreenEvent::Snapshot { id: 3, .. })));
    let capture = handle.snapshot_blocking(0).unwrap();
    assert_eq!(rows_text(&capture), vec!["ok", "", ""]);
    handle.shutdown_blocking().unwrap();
    assert_eq!(events.blocking_recv(), None);
    assert!(matches!(
        handle.feed_blocking(bytes(b"x"), Seq::ZERO),
        Err(ScreenError::Closed { .. })
    ));
}

#[test]
fn a_thread_name_with_a_nul_byte_is_a_spawn_error() {
    let result = ScreenActor::spawn("bad\0name", FakeScreen::factory(size(4, 1)), size(4, 1));
    assert!(matches!(result, Err(ScreenError::Spawn { name, .. }) if name == "bad\0name"));
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn handles_events_and_their_futures_can_move_between_tasks() {
    fn shared<T: Send + Sync + Clone>() {}
    shared::<ScreenHandle>();
    let (handle, mut events) = spawn_fake(4, 1);
    assert_send(&events);
    // tokio::spawn needs Send futures; the PTY reader and writer tasks are spawned.
    assert_send(&handle.feed(bytes(b"x"), Seq::ZERO));
    assert_send(&handle.resize(size(4, 1)));
    assert_send(&handle.snapshot(0));
    assert_send(&handle.request_snapshot(1, 0));
    assert_send(&handle.shutdown());
    assert_send(&events.recv());
}

/// What a [`GatedScreen`] does once its gate opens.
enum Gate {
    Go,
    Panic,
}

/// A screen whose feed of `wait` blocks until the test opens the gate, so a test can
/// hold the actor still and fill its command queue deterministically.
struct GatedScreen {
    entered: mpsc::UnboundedSender<()>,
    gate: std_mpsc::Receiver<Gate>,
    _not_send: Rc<()>,
}

impl Screen for GatedScreen {
    fn feed(&mut self, bytes: &[u8], _sink: &mut dyn ScreenSink) {
        if bytes == b"wait" {
            self.entered.send(()).unwrap();
            if let Ok(Gate::Panic) = self.gate.recv() {
                panic!("the gated backend failed on purpose");
            }
        }
    }

    fn resize(&mut self, _cols: u16, _rows: u16, _sink: &mut dyn ScreenSink) {}

    fn snapshot(&mut self, _scrollback_rows: usize) -> ScreenSnapshot {
        ScreenSnapshot::default()
    }
}

/// A gated screen with its command queue full and the actor held inside `feed`.
async fn gated_and_full() -> (ScreenHandle, ScreenEvents, std_mpsc::Sender<Gate>) {
    let (entered_tx, mut entered) = mpsc::unbounded_channel();
    let (gate, gate_rx) = std_mpsc::channel();
    let factory =
        move || GatedScreen { entered: entered_tx, gate: gate_rx, _not_send: Rc::new(()) };
    let (handle, events) = ScreenActor::spawn("screen-gated", factory, size(4, 1)).unwrap();
    handle.feed(bytes(b"wait"), Seq::ZERO).await.unwrap();
    entered.recv().await.unwrap();
    for index in 0..COMMAND_CAPACITY as u64 {
        handle.feed(bytes(b"x"), Seq::new(4 + index)).await.unwrap();
    }
    (handle, events, gate)
}

#[tokio::test]
async fn a_full_queue_parks_the_async_sender_until_the_screen_takes_a_command() {
    let (handle, _events, gate) = gated_and_full().await;
    let mut parked = pin!(handle.feed(bytes(b"y"), Seq::new(100)));
    assert!(poll_once(parked.as_mut()).await.is_pending());
    gate.send(Gate::Go).unwrap();
    parked.await.unwrap();
    assert_eq!(handle.snapshot(0).await.unwrap().at, Seq::new(101));
}

#[tokio::test]
async fn a_parked_sender_is_woken_when_the_backend_panics() {
    let (handle, mut events, gate) = gated_and_full().await;
    let mut parked = pin!(handle.feed(bytes(b"y"), Seq::new(100)));
    assert!(poll_once(parked.as_mut()).await.is_pending());
    gate.send(Gate::Panic).unwrap();
    assert!(matches!(parked.await, Err(ScreenError::Closed { .. })));
    assert_eq!(events.recv().await, None);
    assert!(matches!(handle.snapshot(0).await, Err(ScreenError::Closed { .. })));
}

/// What [`IdleScreen`] did, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Feed,
    Idle,
}

/// A screen that writes down its feeds and idle steps. Each quiet moment gives it
/// `steps` idle steps: all but the last ask for one more.
struct IdleScreen {
    log: std_mpsc::Sender<Step>,
    steps: usize,
    left: usize,
    _not_send: Rc<()>,
}

impl Screen for IdleScreen {
    fn feed(&mut self, _bytes: &[u8], _sink: &mut dyn ScreenSink) {
        self.left = self.steps;
        self.log.send(Step::Feed).unwrap();
    }

    fn resize(&mut self, _cols: u16, _rows: u16, _sink: &mut dyn ScreenSink) {}

    fn snapshot(&mut self, _scrollback_rows: usize) -> ScreenSnapshot {
        ScreenSnapshot::default()
    }
    fn idle_delay(&self) -> Option<Duration> {
        Some(Duration::from_millis(1))
    }

    fn idle(&mut self) -> Option<Duration> {
        self.log.send(Step::Idle).unwrap();
        self.left = self.left.saturating_sub(1);
        (self.left > 0).then_some(Duration::from_millis(1))
    }
}

fn spawn_idle(steps: usize) -> (ScreenHandle, ScreenEvents, std_mpsc::Receiver<Step>) {
    let (log, steps_rx) = std_mpsc::channel();
    let factory = move || IdleScreen { log, steps, left: steps, _not_send: Rc::new(()) };
    let (handle, events) = ScreenActor::spawn("screen-idle", factory, size(4, 1)).unwrap();
    (handle, events, steps_rx)
}

/// The steps that `log` gets until it is quiet for 200 ms.
fn steps_until_quiet(log: &std_mpsc::Receiver<Step>) -> Vec<Step> {
    std::iter::from_fn(|| log.recv_timeout(Duration::from_millis(200)).ok()).collect()
}

#[test]
fn a_quiet_screen_gets_its_idle_steps_and_then_waits() {
    let (handle, _events, log) = spawn_idle(3);
    assert_eq!(steps_until_quiet(&log), vec![Step::Idle; 3]);
    handle.feed_blocking(bytes(b"x"), Seq::ZERO).unwrap();
    assert_eq!(steps_until_quiet(&log), vec![Step::Feed, Step::Idle, Step::Idle, Step::Idle]);
}

#[test]
fn a_command_during_the_idle_steps_is_served_first() {
    // Steps for far longer than the test runs, one each millisecond.
    let (handle, _events, log) = spawn_idle(usize::MAX);
    assert_eq!(log.recv().unwrap(), Step::Idle);
    handle.feed_blocking(bytes(b"x"), Seq::ZERO).unwrap();
    assert!(log.iter().any(|step| step == Step::Feed));
    assert_eq!(handle.snapshot_blocking(0).unwrap().at, Seq::new(1));
}

#[test]
fn a_screen_without_an_idle_delay_gets_no_idle_step() {
    // The fake screen keeps the default: `idle_delay` is `None`.
    assert_eq!(FakeScreen::new(size(4, 1)).idle_delay(), None);
}
