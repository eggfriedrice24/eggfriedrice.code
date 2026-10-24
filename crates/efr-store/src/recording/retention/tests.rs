use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{ConversationId, Event};
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::recording::{RecordingWriter, segments};
use crate::testing::{self, TestClock};
use crate::{Batch, Store};

const END: Seq = Seq::new(u64::MAX);
const DAY: Duration = Duration::from_secs(24 * 3600);
const AGE: Duration = Duration::from_secs(14 * 24 * 3600);
/// Two 30-byte chunks (46 bytes each with the header) fill a segment of 100 bytes.
const LIMIT: u64 = 100;
/// The file size of a full segment.
const FULL: u64 = 92;

struct Setup {
    dir: TempDir,
    store: Store,
    recordings: Recordings,
    clock: Arc<TestClock>,
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let store = Store::open_in_memory(clock.clone()).await.unwrap();
    let recordings = Recordings::new(
        dir.path().join("recordings"),
        store.writer().clone(),
        store.readers().clone(),
        clock.clone(),
    )
    .with_segment_limit(LIMIT);
    Setup { dir, store, recordings, clock }
}

fn conversation(pty: u64) -> ConversationId {
    testing::conversation(pty)
}

impl Setup {
    fn root(&self) -> PathBuf {
        self.dir.path().join("recordings")
    }

    async fn append(&self, conversation: ConversationId, event: Event) {
        self.store.writer().append(Batch::new().event(conversation, event)).await.unwrap();
    }

    /// The shell of PTY `n` starts in a conversation of its own.
    async fn started(&self, n: u64) -> PtyId {
        let pty_id = testing::pty(n);
        self.append(conversation(n), testing::created(None)).await;
        let started = Event::ShellStarted { pty_id, cwd: testing::path("/"), pid: Some(1) };
        self.append(conversation(n), started).await;
        pty_id
    }

    async fn exited(&self, n: u64) {
        let exited = Event::ShellExited { pty_id: testing::pty(n), exit_code: Some(0) };
        self.append(conversation(n), exited).await;
    }

    /// A writer of `pty_id` that wrote `chunks` chunks of 30 bytes: one segment for each
    /// two chunks.
    async fn write(&self, pty_id: PtyId, chunks: u8) -> RecordingWriter {
        let mut writer = self.recordings.start(pty_id).await.unwrap();
        for n in 0..chunks {
            writer.append(&[n; 30]).await.unwrap();
            self.clock.advance(Duration::from_secs(1));
        }
        writer
    }

    /// The start offsets of the segment rows of `pty_id`.
    async fn starts(&self, pty_id: PtyId) -> Vec<u64> {
        let all = self.store.readers().with(move |conn| segments(conn, pty_id)).await.unwrap();
        all.iter().map(|segment| segment.start_seq.get()).collect()
    }

    /// The start offsets of the segment files of `pty_id`.
    fn files(&self, pty_id: PtyId) -> Vec<u64> {
        let mut starts: Vec<u64> = list_files(&self.root())
            .unwrap()
            .into_iter()
            .filter(|file| file.pty_id == pty_id)
            .map(|file| file.start_seq)
            .collect();
        starts.sort_unstable();
        starts
    }

    async fn collect(&self, retention: Retention) -> Collected {
        self.recordings.collect(retention).await.unwrap()
    }
}

fn by_age() -> Retention {
    Retention::new(Some(AGE), None, 1024 * 1024)
}

fn by_size(max_bytes: u64, live_tail: u64) -> Retention {
    Retention::new(None, Some(max_bytes), live_tail)
}

#[tokio::test]
async fn an_exited_shell_goes_whole_after_the_age_and_reads_with_a_later_start() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    setup.write(pty, 5).await.close().await.unwrap();
    setup.exited(1).await;
    setup.clock.advance(AGE);
    assert_eq!(setup.collect(by_age()).await.segments, 0, "exactly 14 days is not more");

    setup.clock.advance(Duration::from_secs(1));
    let collected = setup.collect(by_age()).await;

    assert_eq!(
        (collected.segments, collected.ptys, collected.freed, collected.held),
        (3, 1, 2 * FULL + 46, 0)
    );
    assert_eq!(setup.files(pty), Vec::<u64>::new());
    assert!(!setup.root().join(pty.to_string()).exists(), "the empty directory goes too");
    assert_eq!(setup.starts(pty).await, [150], "one empty row stays after the last byte");
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!((range.start, range.end, range.chunks), (Seq::new(150), Seq::new(150), vec![]));
    // The next pass finds nothing to do.
    assert_eq!(setup.collect(by_age()).await, Collected::default());
}

#[tokio::test]
async fn a_shell_that_exited_recently_or_still_runs_keeps_its_recording() {
    let setup = setup().await;
    let running = setup.started(1).await;
    let mut writer = setup.write(running, 3).await;
    let recent = setup.started(2).await;
    setup.write(recent, 3).await.close().await.unwrap();
    setup.clock.advance(AGE);
    setup.exited(2).await;
    setup.clock.advance(DAY);
    writer.append(b"later").await.unwrap();

    let collected = setup.collect(by_age()).await;

    assert_eq!((collected.segments, collected.ptys), (0, 0));
    assert_eq!(setup.starts(running).await, [0, 60]);
    assert_eq!(setup.starts(recent).await, [0, 60]);
    drop(writer);
}

#[tokio::test]
async fn a_pty_that_the_shells_do_not_know_is_never_aged() {
    let setup = setup().await;
    let pty = testing::pty(7);
    setup.write(pty, 3).await.close().await.unwrap();
    setup.clock.advance(AGE * 10);

    let collected = setup.collect(by_age()).await;

    assert_eq!(collected.segments, 0);
    assert_eq!(setup.starts(pty).await, [0, 60]);
}

#[tokio::test]
async fn the_limit_takes_the_oldest_segments_first_across_ptys() {
    let setup = setup().await;
    let old = setup.started(1).await;
    setup.write(old, 4).await.close().await.unwrap();
    let new = setup.started(2).await;
    setup.write(new, 4).await.close().await.unwrap();
    setup.exited(1).await;
    setup.exited(2).await;
    setup.clock.advance(WRITER_GRACE + Duration::from_secs(1));

    // Four full segments; room for two.
    let collected = setup.collect(by_size(2 * FULL, 1024 * 1024)).await;

    assert_eq!((collected.segments, collected.ptys, collected.held), (2, 1, 2 * FULL));
    assert_eq!(setup.starts(old).await, [120], "the old PTY went whole");
    assert_eq!(setup.starts(new).await, [0, 60]);
    assert_eq!(setup.files(new), [0, 60]);

    // One more segment of room less takes the oldest segment of the new PTY.
    let collected = setup.collect(by_size(FULL, 1024 * 1024)).await;
    assert_eq!((collected.segments, collected.ptys), (1, 0));
    assert_eq!(setup.starts(new).await, [60]);
    let range = setup.recordings.read_range(new, Seq::ZERO, END).await.unwrap();
    assert_eq!(range.start, Seq::new(60), "what is left reads with a later start");
    assert_eq!(range.bytes(), [[2; 30], [3; 30]].concat());
}

#[tokio::test]
async fn a_running_shell_keeps_its_newest_segment_and_its_tail_past_the_limit() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    let writer = setup.write(pty, 9).await;
    assert_eq!(setup.starts(pty).await, [0, 60, 120, 180, 240]);

    // The newest segment is open and counts as empty: 100 bytes of tail keep the two
    // closed segments before it, 120..240.
    let collected = setup.collect(by_size(0, 100)).await;

    assert_eq!(collected.segments, 2);
    assert_eq!(setup.starts(pty).await, [120, 180, 240]);
    assert_eq!(setup.files(pty), [120, 180, 240]);
    // Without a tail, only the newest stays.
    setup.collect(by_size(0, 0)).await;
    assert_eq!(setup.starts(pty).await, [240]);
    assert_eq!(setup.files(pty), [240]);

    let mut writer = writer;
    writer.append(&[9; 30]).await.unwrap();
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!((range.start, range.end), (Seq::new(240), Seq::new(300)));
}

#[tokio::test]
async fn a_reopened_last_segment_is_never_deleted() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    setup.write(pty, 3).await.close().await.unwrap();
    // A new writer continues the closed segment that has room and opens it again, as
    // for the output that the reader drains after an exit.
    let mut writer = setup.recordings.start(pty).await.unwrap();
    setup.exited(1).await;
    writer.append(b"drained").await.unwrap();

    let collected = setup.collect(Retention::new(Some(Duration::ZERO), Some(0), 0)).await;

    assert_eq!(collected.segments, 1, "only the closed segment before it went");
    assert_eq!(setup.starts(pty).await, [60]);
    assert_eq!(setup.files(pty), [60]);
    writer.close().await.unwrap();
    let range = setup.recordings.read_range(pty, Seq::new(60), END).await.unwrap();
    assert_eq!(range.bytes(), [&[2; 30][..], b"drained"].concat());
}

#[tokio::test]
async fn a_crash_between_the_rows_and_the_files_leaves_orphans_that_the_next_pass_removes() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    setup.write(pty, 4).await.close().await.unwrap();
    setup.exited(1).await;
    setup.clock.advance(AGE + DAY);

    // The rows go, and the daemon stops before the unlink.
    let pruned = setup.recordings.prune_index(by_age()).await.unwrap();
    assert_eq!(pruned.deleted.len(), 2);
    assert_eq!(setup.starts(pty).await, [120]);
    assert_eq!(setup.files(pty), [0, 60], "only orphans are left");

    let collected = setup.collect(by_age()).await;

    assert_eq!((collected.segments, collected.orphans, collected.freed), (0, 2, 2 * FULL));
    assert_eq!(setup.files(pty), Vec::<u64>::new());
    assert_eq!(setup.starts(pty).await, [120]);
}

#[tokio::test]
async fn only_segment_files_without_a_row_are_orphans() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    let writer = setup.write(pty, 1).await;
    let dir = setup.root().join(pty.to_string());
    fs::write(dir.join("600.rec"), b"left behind").unwrap();
    fs::write(dir.join("notes.txt"), b"not a segment").unwrap();
    fs::write(dir.join("0600.rec"), b"not the writer's name").unwrap();
    fs::create_dir(setup.root().join("not-a-pty")).unwrap();
    fs::write(setup.root().join("not-a-pty").join("0.rec"), b"not a pty").unwrap();
    std::os::unix::fs::symlink(dir.join("0.rec"), dir.join("700.rec")).unwrap();

    let collected = setup.collect(Retention::new(None, None, 0)).await;

    assert_eq!((collected.segments, collected.orphans), (0, 1));
    assert!(!dir.join("600.rec").exists());
    for kept in ["0.rec", "notes.txt", "0600.rec"] {
        assert!(dir.join(kept).exists(), "{kept}");
    }
    assert!(fs::symlink_metadata(dir.join("700.rec")).is_ok(), "a link is never followed");
    assert!(setup.root().join("not-a-pty").join("0.rec").exists());
    drop(writer);
}

#[tokio::test]
async fn a_writer_of_a_collected_pty_goes_on_after_its_last_byte() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    setup.write(pty, 3).await.close().await.unwrap();
    setup.exited(1).await;
    setup.clock.advance(AGE + DAY);
    setup.collect(by_age()).await;

    let mut writer = setup.recordings.start(pty).await.unwrap();
    let seq = writer.append(b"more").await.unwrap();

    assert_eq!(seq, Seq::new(90));
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!((range.start, range.bytes()), (Seq::new(90), b"more".to_vec()));
}

#[tokio::test]
async fn an_open_segment_of_a_shell_that_died_with_the_daemon_ends_at_its_last_chunk() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    // A crash: the writer never closes its segment.
    drop(setup.write(pty, 3).await);
    setup.exited(1).await;
    setup.clock.advance(AGE + DAY);

    let collected = setup.collect(by_age()).await;

    assert_eq!((collected.segments, collected.ptys), (2, 1));
    assert_eq!(setup.starts(pty).await, [90]);
}

#[tokio::test]
async fn nothing_goes_when_both_limits_are_off() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    setup.write(pty, 6).await.close().await.unwrap();
    setup.exited(1).await;
    setup.clock.advance(AGE * 100);

    let collected = setup.collect(Retention::new(None, None, 0)).await;

    assert_eq!(collected, Collected { held: 3 * FULL, ..Collected::default() });
    assert_eq!(setup.starts(pty).await, [0, 60, 120]);
}

#[tokio::test]
async fn the_rows_are_read_in_key_order_without_a_sort() {
    let setup = setup().await;
    let plan: Vec<String> = setup
        .store
        .readers()
        .with(|conn| {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {ROWS}"))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(3))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap();
    assert!(plan.iter().all(|step| !step.contains("TEMP B-TREE")), "{plan:?}");
    assert!(plan.iter().any(|step| step.starts_with("SEARCH s")), "{plan:?}");
}

/// A row of PTY `pty` at `start` with `bytes`, closed at `closed` seconds.
fn row(pty: u64, start: u64, bytes: u64, closed: Option<i64>, shell: Shell) -> SegmentRow {
    let at = |secs: i64| testing::start() + SignedDuration::from_secs(secs);
    SegmentRow {
        segment: Segment {
            pty_id: testing::pty(pty),
            start_seq: Seq::new(start),
            path: relative_path(testing::pty(pty), start),
            bytes,
            started_at: at(0),
            closed_at: closed.map(at),
        },
        shell: match shell {
            Shell::Unknown => None,
            Shell::Running => Some((ShellStatus::Running, None)),
            Shell::ExitedAt(secs) => Some((ShellStatus::Exited, Some(at(secs)))),
        },
    }
}

#[derive(Clone, Copy)]
enum Shell {
    Unknown,
    Running,
    ExitedAt(i64),
}

fn file(row: &SegmentRow, len: u64) -> SegmentFile {
    SegmentFile { pty_id: row.segment.pty_id, start_seq: row.segment.start_seq.get(), len }
}

#[test]
fn the_plan_never_takes_the_newest_segment_of_a_pty_that_can_write() {
    let now = testing::start() + SignedDuration::from_hours(24 * 365);
    let grace = i64::try_from(WRITER_GRACE.as_secs()).unwrap();
    let just_exited = now.duration_since(testing::start()).as_secs() - grace + 1;
    for shell in [Shell::Unknown, Shell::Running, Shell::ExitedAt(just_exited)] {
        let rows = [row(1, 0, 10, Some(1), shell), row(1, 10, 10, Some(2), shell)];
        let files: Vec<SegmentFile> = rows.iter().map(|row| file(row, 50)).collect();
        let all = Retention::new(Some(Duration::ZERO), Some(0), 0);

        let plan = plan(&rows, &files, &all, now, |_| unreachable!());

        let starts: Vec<u64> = plan.doomed.iter().map(|doomed| doomed.start_seq).collect();
        assert_eq!(starts, [0]);
        assert!(plan.doomed.iter().all(|doomed| doomed.writable));
        assert_eq!((plan.ends, plan.held), (vec![], 50));
    }
}

#[tokio::test]
async fn the_writer_transaction_never_deletes_the_newest_or_an_open_row_of_a_pty_that_can_write() {
    let setup = setup().await;
    let pty = setup.started(1).await;
    let writer = setup.write(pty, 5).await;
    // A plan that went wrong: every row, the open newest one too.
    let doomed: Vec<Doomed> = [0, 60, 120]
        .into_iter()
        .map(|start_seq| Doomed { pty_id: pty, start_seq, writable: true })
        .collect();

    let (deleted, whole) = testing::on_writer(setup.store.writer(), move |conn| {
        prune(conn, &doomed, &[], testing::start())
    })
    .await
    .unwrap();

    assert_eq!((deleted, whole), (vec![(pty, 0), (pty, 60)], vec![]));
    assert_eq!(setup.starts(pty).await, [120]);
    drop(writer);
}

mod perf;
