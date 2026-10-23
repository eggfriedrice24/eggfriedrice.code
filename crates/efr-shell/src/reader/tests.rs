use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use efr_holder::Size;
use efr_protocol::{PtyId, Seq};
use efr_screen::{ScreenEvent, ScreenEvents, ShellMarkKind, ShellMarkScanner};
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{BATCH, ReaderTargets, master, read_loop};
use crate::ScreenFactory;
use crate::session::Msg;
use crate::testing::{Recorded, Vt100Screens};

/// Long enough for a chunk that is already there, on a busy machine.
const WAIT: Duration = Duration::from_secs(10);

fn pty_id() -> PtyId {
    "01920000-0000-7000-8000-0000000000aa".parse().unwrap()
}

/// A reader over one end of a socketpair, as over a PTY master. The test writes to
/// the other end and reads what the session gets.
struct Reader {
    shell: Option<UnixStream>,
    recorded: Arc<Recorded>,
    session: mpsc::Receiver<Msg>,
    events: ScreenEvents,
    task: JoinHandle<()>,
}

impl Reader {
    fn start() -> Reader {
        let (ours, shell) = UnixStream::pair().unwrap();
        let master = master(OwnedFd::from(ours)).unwrap();
        let recorded = Arc::new(Recorded::default());
        let (session, inbox) = mpsc::channel(4096);
        let (screen, events) =
            Vt100Screens.spawn("reader-test", Size { cols: 80, rows: 24 }).unwrap();
        let targets = ReaderTargets {
            pty_id: pty_id(),
            recording: Arc::clone(&recorded) as _,
            session,
            screen,
        };
        let task = tokio::spawn(read_loop(master, targets));
        Reader { shell: Some(shell), recorded, session: inbox, events, task }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.shell.as_mut().unwrap().write_all(bytes).unwrap();
    }

    /// The next chunk that the session gets.
    async fn chunk(&mut self) -> (Seq, Bytes) {
        match tokio::time::timeout(WAIT, self.session.recv()).await {
            Ok(Some(Msg::Chunk { start, bytes })) => (start, bytes),
            Ok(_) => panic!("the session got no chunk"),
            Err(_) => panic!("no chunk came within {WAIT:?}"),
        }
    }

    /// Closes the shell's end and returns every chunk that the session got after the
    /// ones read so far, once the reader has ended.
    async fn end(mut self) -> (Vec<(Seq, Bytes)>, Arc<Recorded>) {
        drop(self.shell.take());
        tokio::time::timeout(WAIT, &mut self.task).await.unwrap().unwrap();
        let mut chunks = Vec::new();
        while let Ok(msg) = self.session.try_recv() {
            if let Msg::Chunk { start, bytes } = msg {
                chunks.push((start, bytes));
            }
        }
        (chunks, self.recorded)
    }
}

#[tokio::test]
async fn every_byte_arrives_once_in_order_at_back_to_back_offsets() {
    let mut reader = Reader::start();
    let mut written = Vec::new();
    // Pieces of many sizes, some far larger than one batch.
    for (n, size) in [1, 7, 4096, 3000, 70_000, 13, 200_000, 65_536, 65_537].iter().enumerate() {
        written.extend((0..*size).map(|i| b'a' + ((i + n) % 26) as u8));
    }
    // A thread writes, because a full socket blocks the writer until the reader reads.
    let mut shell = reader.shell.take().unwrap();
    let bytes = written.clone();
    let writer = std::thread::spawn(move || shell.write_all(&bytes).unwrap());
    let (chunks, recorded) = reader.end().await;
    writer.join().unwrap();

    let mut stream = Vec::new();
    for (start, bytes) in &chunks {
        assert_eq!(start.get(), stream.len() as u64, "the chunks are back to back");
        assert!(!bytes.is_empty() && bytes.len() <= BATCH, "a chunk of {} bytes", bytes.len());
        stream.extend_from_slice(bytes);
    }
    assert_eq!(stream.len(), written.len());
    assert!(stream == written, "the session got the bytes in order");
    // The recording got the same chunks, before the session.
    assert!(recorded.stream(pty_id()) == written, "the recording got the bytes in order");
}

#[tokio::test]
async fn a_prompt_arrives_without_waiting_for_more_output() {
    let mut reader = Reader::start();
    reader.write(b"\x1b]133;A\x07% ");
    // The shell waits for input now: nothing more comes, and the reader hands on what
    // it has at once.
    assert_eq!(reader.chunk().await, (Seq::ZERO, Bytes::from_static(b"\x1b]133;A\x07% ")));
    reader.write(b"ls\r\n");
    assert_eq!(reader.chunk().await, (Seq::new(10), Bytes::from_static(b"ls\r\n")));
    let (rest, _) = reader.end().await;
    assert_eq!(rest, Vec::new());
}

#[tokio::test]
async fn the_bytes_before_the_end_of_the_output_are_handed_on() {
    let mut reader = Reader::start();
    // The end comes in the same batch as the last bytes.
    reader.write(b"exit\r\n");
    let (chunks, _) = reader.end().await;
    assert_eq!(chunks, vec![(Seq::ZERO, Bytes::from_static(b"exit\r\n"))]);
}

#[tokio::test]
async fn a_mark_split_between_two_chunks_is_found() {
    let mut reader = Reader::start();
    reader.write(b"out\x1b]133;D");
    let first = reader.chunk().await;
    assert_eq!(first, (Seq::ZERO, Bytes::from_static(b"out\x1b]133;D")));
    reader.write(b";0\x07\x1b]133;A\x07");
    let second = reader.chunk().await;
    assert_eq!(second.0, Seq::new(10), "the second chunk starts where the first ended");

    // The session's scanner, fed chunk by chunk, finds both marks.
    let mut scanner = ShellMarkScanner::new();
    let mut marks = scanner.scan(&first.1, first.0);
    marks.extend(scanner.scan(&second.1, second.0));
    let spans: Vec<(u64, u64)> =
        marks.iter().map(|mark| (mark.start.get(), mark.end.get())).collect();
    assert_eq!(spans, vec![(3, 13), (13, 21)]);
    assert!(matches!(marks[0].kind, ShellMarkKind::SemanticPrompt(_)));

    // So does the screen.
    let mut screen_marks = Vec::new();
    while screen_marks.len() < 2 {
        match tokio::time::timeout(WAIT, reader.events.recv()).await.unwrap() {
            Some(ScreenEvent::ShellMark(mark)) => screen_marks.push(mark),
            Some(_) => {}
            None => panic!("the screen stopped"),
        }
    }
    assert_eq!(screen_marks, marks);
    reader.end().await;
}
