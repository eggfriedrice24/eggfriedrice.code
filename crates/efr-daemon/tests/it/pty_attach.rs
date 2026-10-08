//! `pty.attach`, `pty.write` and `pty.resize` over a `TestDaemon`: sequence numbers are
//! offsets in the PTY's recording, a resume replays from its offset, a fresh attach
//! starts with the screen, and live output follows without a hole or a repeat.

use efr_protocol::{
    Base64Bytes, ErrorCode, Method, PtyAttach, PtyAttachItem, PtyId, PtyResize, PtyResizeResult,
    PtyWrite, PtyWriteResult, Seq, Size,
};
use efr_test_daemon::{ClientError, ItemStream, PROMPT, Replay, command_output};
use efr_test_support::Wait;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

/// Everything the scenario's fake shell printed: the prompt, then `seq 1 5` and the
/// next prompt.
fn printed() -> Vec<u8> {
    let mut bytes = PROMPT.to_vec();
    bytes.extend_from_slice(&command_output(b"1\r\n2\r\n3\r\n4\r\n5\r\n", 0));
    bytes
}

fn pty_id(replay: &Replay) -> PtyId {
    replay.bindings().get("<pty:1>").unwrap().parse().unwrap()
}

async fn attach(replay: &Replay, since: Option<u64>) -> ItemStream<PtyAttachItem> {
    let params =
        PtyAttach { pty_id: pty_id(replay), since_seq: since.map(Seq::new), scrollback_rows: None };
    replay.client().stream(Method::PtyAttach(params)).await.unwrap()
}

/// The next item of `stream`. A lost item fails the test at the wait's limit and does
/// not hang it.
async fn next(stream: &mut ItemStream<PtyAttachItem>, what: &str) -> PtyAttachItem {
    let next = Wait::new(what).until_some_async(async || Some(stream.next().await)).await;
    next.unwrap().unwrap_or_else(|| panic!("the stream ended before {what}")).unwrap()
}

/// Output items from `start` until `len` bytes arrived, checking they are back to back.
async fn output(stream: &mut ItemStream<PtyAttachItem>, start: u64, len: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while bytes.len() < len {
        match next(stream, "output").await {
            PtyAttachItem::Output { seq, data } => {
                assert_eq!(seq.get(), start + bytes.len() as u64, "no hole and no repeat");
                bytes.extend_from_slice(data.as_bytes());
            }
            other => panic!("expected output, got {other:?}"),
        }
    }
    bytes
}

#[tokio::test]
async fn pty_attach_since_seq() {
    let mut replay = Replay::run("pty_attach_since_seq").await.unwrap();
    let printed = printed();
    let end = printed.len() as u64;
    let middle = PROMPT.len() as u64;

    let mut from_start = attach(&replay, Some(0)).await;
    assert_eq!(output(&mut from_start, 0, printed.len()).await, printed);
    let mut from_middle = attach(&replay, Some(middle)).await;
    assert_eq!(
        output(&mut from_middle, middle, printed.len() - PROMPT.len()).await,
        &printed[PROMPT.len()..]
    );
    let mut fresh = attach(&replay, None).await;
    match next(&mut fresh, "the screen").await {
        PtyAttachItem::Snapshot { seq, snapshot } => {
            assert_eq!(seq.get(), end, "the screen has seen every recorded byte");
            assert!(snapshot.size.cols > 0 && snapshot.size.rows > 0);
        }
        other => panic!("an attach without since_seq starts with the screen: {other:?}"),
    }

    // Live output reaches every attached client at the next offset.
    replay.terminal("shell").await.unwrap().print(b"more\r\n").await.unwrap();
    for stream in [&mut from_start, &mut from_middle, &mut fresh] {
        assert_eq!(output(stream, end, 6).await, b"more\r\n");
    }
    drop((from_start, from_middle, fresh));
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn pty_write_reaches_the_shell_and_pty_resize_the_holder_and_the_watchers() {
    let mut replay = Replay::run("pty_attach_since_seq").await.unwrap();
    let pty_id = pty_id(&replay);
    // `stream` returns when the request is sent, and the daemon runs the requests of one
    // connection side by side. Only the first item shows that the watcher gets live
    // steps: the daemon registers it before it sends anything. So the watcher resumes
    // from the start and reads the recording before the resize.
    let printed = printed();
    let mut watcher = attach(&replay, Some(0)).await;
    assert_eq!(output(&mut watcher, 0, printed.len()).await, printed);

    let write = Method::PtyWrite(PtyWrite { pty_id, data: Base64Bytes::new(b"q".to_vec()) });
    let _: PtyWriteResult = replay.client().call(write).await.unwrap();
    assert_eq!(replay.terminal("shell").await.unwrap().typed_exact(1).await.unwrap(), b"q");

    let size = Size { cols: 100, rows: 30 };
    let _: PtyResizeResult =
        replay.client().call(Method::PtyResize(PtyResize { pty_id, size })).await.unwrap();
    assert_eq!(replay.daemon().holder().unwrap().resizes(), [(pty_id, size)]);
    match next(&mut watcher, "the resize").await {
        PtyAttachItem::Resized { size: resized, .. } => assert_eq!(resized, size),
        other => panic!("a watcher hears the resize: {other:?}"),
    }
    drop(watcher);
    replay.stop().await.unwrap();
}

async fn resize(replay: &Replay, size: Size) {
    let params = PtyResize { pty_id: pty_id(replay), size };
    let _: PtyResizeResult = replay.client().call(Method::PtyResize(params)).await.unwrap();
}

#[tokio::test]
async fn a_resume_replays_the_resizes_it_missed_at_their_place() {
    let mut replay = Replay::run("pty_attach_since_seq").await.unwrap();
    let printed = printed();
    let end = printed.len() as u64;
    let wide = Size { cols: 120, rows: 40 };
    let narrow = Size { cols: 90, rows: 20 };

    // While no client is attached: a resize, output, and another resize.
    resize(&replay, wide).await;
    replay.terminal("shell").await.unwrap().print(b"more\r\n").await.unwrap();
    // The output is stored before the second resize: a client that resumes after the
    // first resize gets it.
    let mut after_first = attach(&replay, Some(end)).await;
    assert_eq!(next(&mut after_first, "the first resize").await, resized(end, wide));
    assert_eq!(output(&mut after_first, end, 6).await, b"more\r\n");
    drop(after_first);
    resize(&replay, narrow).await;

    // A client that resumes from the middle gets the output and both resizes in the order
    // of the recording.
    let middle = PROMPT.len() as u64;
    let mut resumed = attach(&replay, Some(middle)).await;
    assert_eq!(
        output(&mut resumed, middle, printed.len() - PROMPT.len()).await,
        &printed[PROMPT.len()..]
    );
    assert_eq!(next(&mut resumed, "the first resize").await, resized(end, wide));
    assert_eq!(output(&mut resumed, end, 6).await, b"more\r\n");
    assert_eq!(next(&mut resumed, "the second resize").await, resized(end + 6, narrow));

    // Live steps follow, each once.
    resize(&replay, wide).await;
    assert_eq!(next(&mut resumed, "a live resize").await, resized(end + 6, wide));
    replay.terminal("shell").await.unwrap().print(b"last\r\n").await.unwrap();
    assert_eq!(output(&mut resumed, end + 6, 6).await, b"last\r\n");

    // A fresh attach starts with the screen at its size, not with the resizes.
    let mut fresh = attach(&replay, None).await;
    match next(&mut fresh, "the screen").await {
        PtyAttachItem::Snapshot { seq, snapshot } => {
            assert_eq!(seq.get(), end + 12);
            assert_eq!(snapshot.size, wide);
        }
        other => panic!("an attach without since_seq starts with the screen: {other:?}"),
    }
    drop((resumed, fresh));
    replay.stop().await.unwrap();
}

fn resized(seq: u64, size: Size) -> PtyAttachItem {
    PtyAttachItem::Resized { seq: Seq::new(seq), size }
}

#[tokio::test]
async fn attaching_to_a_pty_that_does_not_exist_is_not_found() {
    let replay = Replay::run("single_turn_text").await.unwrap();
    let unknown = "0192f0c1-7a00-7000-8000-00000000dead".parse().unwrap();

    let params = PtyAttach { pty_id: unknown, since_seq: Some(Seq::ZERO), scrollback_rows: None };
    let mut stream =
        replay.client().stream::<PtyAttachItem>(Method::PtyAttach(params)).await.unwrap();
    let ended = stream.next().await.unwrap();

    let Err(ClientError::Server { body }) = ended else { panic!("{ended:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    drop(stream);
    replay.stop().await.unwrap();
}
