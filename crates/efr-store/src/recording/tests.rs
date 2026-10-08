use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::Store;
use crate::testing::{self, TestClock};

const END: Seq = Seq::new(u64::MAX);

struct Setup {
    dir: TempDir,
    // Kept so the store outlives the test's recordings.
    _store: Store,
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
    );
    Setup { dir, _store: store, recordings, clock }
}

impl Setup {
    async fn segments(&self, pty_id: PtyId) -> Vec<Segment> {
        self.recordings.readers.with(move |conn| segments(conn, pty_id)).await.unwrap()
    }

    fn file(&self, pty_id: PtyId, start_seq: u64) -> PathBuf {
        self.dir.path().join("recordings").join(relative_path(pty_id, start_seq))
    }
}

fn micros(at: Timestamp) -> Timestamp {
    sql::truncate_to_micros(at)
}

#[tokio::test]
async fn appended_bytes_read_back_with_their_offsets_and_times() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();

    let first = writer.append(b"hello ").await.unwrap();
    let t0 = micros(setup.clock.now());
    setup.clock.advance(Duration::from_millis(250));
    let second = writer.append(b"world").await.unwrap();
    let t1 = micros(setup.clock.now());
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();

    assert_eq!((first, second, writer.end_seq()), (Seq::new(0), Seq::new(6), Seq::new(11)));
    assert_eq!(
        range,
        RecordedRange {
            start: Seq::new(0),
            end: Seq::new(11),
            chunks: vec![
                RecordedChunk { seq: Seq::new(0), at: t0, data: b"hello ".to_vec() },
                RecordedChunk { seq: Seq::new(6), at: t1, data: b"world".to_vec() },
            ],
            resizes: vec![],
        }
    );
    assert_eq!(range.bytes(), b"hello world");
}

const WIDE: Size = Size { cols: 120, rows: 40 };
const NARROW: Size = Size { cols: 80, rows: 24 };

/// The offsets and sizes of a range's resizes.
fn sizes(range: &RecordedRange) -> Vec<(u64, Size)> {
    range.resizes.iter().map(|resize| (resize.seq.get(), resize.size)).collect()
}

#[tokio::test]
async fn sizes_read_back_at_their_offsets_between_the_chunks() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();

    writer.append(b"hello ").await.unwrap();
    setup.clock.advance(Duration::from_millis(10));
    let wide = writer.resize(WIDE).await.unwrap();
    let t_wide = micros(setup.clock.now());
    writer.append(b"world").await.unwrap();
    let narrow = writer.resize(NARROW).await.unwrap();

    assert_eq!((wide, narrow, writer.end_seq()), (Seq::new(6), Seq::new(11), Seq::new(11)));
    let all = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(all.bytes(), b"hello world", "a size takes no stream bytes");
    assert_eq!((all.start, all.end), (Seq::new(0), Seq::new(11)));
    assert_eq!(sizes(&all), [(6, WIDE), (11, NARROW)]);
    assert_eq!(all.resizes[0].at, t_wide);
    // A size belongs to a range that starts at its offset, not to one that ends there.
    let from_six = setup.recordings.read_range(pty, Seq::new(6), END).await.unwrap();
    assert_eq!(sizes(&from_six), [(6, WIDE), (11, NARROW)]);
    let before = setup.recordings.read_range(pty, Seq::ZERO, Seq::new(6)).await.unwrap();
    assert_eq!(sizes(&before), []);
    let after = setup.recordings.read_range(pty, Seq::new(7), END).await.unwrap();
    assert_eq!(sizes(&after), [(11, NARROW)]);
}

#[tokio::test]
async fn a_size_at_the_end_of_a_segment_reads_from_the_next_offset() {
    let mut setup = setup().await;
    // Two 30-byte chunks and a size (92 + 20 bytes) fit; the next chunk does not.
    setup.recordings = setup.recordings.clone().with_segment_limit(120);
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(&[1; 30]).await.unwrap();
    writer.append(&[2; 30]).await.unwrap();
    writer.resize(WIDE).await.unwrap();
    writer.append(&[3; 30]).await.unwrap();

    let starts: Vec<u64> =
        setup.segments(pty).await.iter().map(|segment| segment.start_seq.get()).collect();
    assert_eq!(starts, [0, 60]);
    assert_eq!(fs::metadata(setup.file(pty, 0)).unwrap().len(), 112);
    let range = setup.recordings.read_range(pty, Seq::new(60), END).await.unwrap();
    assert_eq!(sizes(&range), [(60, WIDE)]);
    assert_eq!(range.bytes(), vec![3; 30]);
}

#[tokio::test]
async fn a_size_that_does_not_fit_starts_the_next_segment() {
    let mut setup = setup().await;
    setup.recordings = setup.recordings.clone().with_segment_limit(100);
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(&[1; 30]).await.unwrap();
    writer.append(&[2; 30]).await.unwrap();
    writer.resize(WIDE).await.unwrap();

    let starts: Vec<u64> =
        setup.segments(pty).await.iter().map(|segment| segment.start_seq.get()).collect();
    assert_eq!(starts, [0, 60]);
    assert_eq!(fs::metadata(setup.file(pty, 60)).unwrap().len(), 20);
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(sizes(&range), [(60, WIDE)]);
}

#[tokio::test]
async fn a_segment_of_sizes_alone_never_rotates() {
    // A rotation there would start a segment at the same offset, which the index
    // cannot hold.
    let mut setup = setup().await;
    setup.recordings = setup.recordings.clone().with_segment_limit(40);
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    for _ in 0..3 {
        writer.resize(WIDE).await.unwrap();
    }
    writer.append(&[1; 30]).await.unwrap();
    writer.append(&[2; 30]).await.unwrap();
    drop(writer);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.resize(NARROW).await.unwrap();
    writer.append(b"next").await.unwrap();

    let starts: Vec<u64> =
        setup.segments(pty).await.iter().map(|segment| segment.start_seq.get()).collect();
    assert_eq!(starts, [0, 30, 60]);
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(sizes(&range), [(0, WIDE), (0, WIDE), (0, WIDE), (60, NARROW)]);
    assert_eq!(range.end, Seq::new(64));
}

#[tokio::test]
async fn a_new_writer_continues_after_sizes() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"ab").await.unwrap();
    writer.resize(WIDE).await.unwrap();
    writer.close().await.unwrap();

    let mut writer = setup.recordings.start(pty).await.unwrap();
    assert_eq!(writer.end_seq(), Seq::new(2), "a size takes no stream bytes");
    writer.append(b"c").await.unwrap();

    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(range.bytes(), b"abc");
    assert_eq!(sizes(&range), [(2, WIDE)]);
}

#[tokio::test]
async fn a_range_is_clipped_to_the_requested_offsets() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"hello ").await.unwrap();
    writer.append(b"world").await.unwrap();

    let range = setup.recordings.read_range(pty, Seq::new(3), Seq::new(8)).await.unwrap();

    assert_eq!((range.start, range.end), (Seq::new(3), Seq::new(8)));
    let seqs: Vec<u64> = range.chunks.iter().map(|chunk| chunk.seq.get()).collect();
    assert_eq!(seqs, [3, 6]);
    assert_eq!(range.bytes(), b"lo wo");
}

#[tokio::test]
async fn a_range_past_the_end_stops_at_what_was_recorded() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"0123456789").await.unwrap();

    let tail = setup.recordings.read_range(pty, Seq::new(5), Seq::new(100)).await.unwrap();
    let caught_up = setup.recordings.read_range(pty, Seq::new(10), END).await.unwrap();
    let ahead = setup.recordings.read_range(pty, Seq::new(50), END).await.unwrap();

    assert_eq!(
        (tail.start, tail.end, tail.bytes()),
        (Seq::new(5), Seq::new(10), b"56789".to_vec())
    );
    assert_eq!(
        (caught_up.start, caught_up.end, caught_up.chunks),
        (Seq::new(10), Seq::new(10), vec![])
    );
    assert_eq!((ahead.start, ahead.end), (Seq::new(50), Seq::new(50)));
}

#[tokio::test]
async fn an_unknown_pty_reads_as_empty() {
    let setup = setup().await;
    let range = setup.recordings.read_range(testing::pty(9), Seq::ZERO, END).await.unwrap();
    assert_eq!((range.start, range.end, range.chunks), (Seq::ZERO, Seq::ZERO, vec![]));
}

#[tokio::test]
async fn segments_rotate_before_the_limit_and_ranges_span_them() {
    let mut setup = setup().await;
    // Room for two 30-byte chunks (46 bytes each with the header) but not three.
    setup.recordings = setup.recordings.clone().with_segment_limit(100);
    let pty = testing::pty(1);
    let stream: Vec<u8> = (0..90u8).collect();
    let mut writer = setup.recordings.start(pty).await.unwrap();
    for part in stream.chunks(30) {
        writer.append(part).await.unwrap();
    }

    let segments = setup.segments(pty).await;
    let range = setup.recordings.read_range(pty, Seq::new(50), Seq::new(70)).await.unwrap();

    let starts: Vec<(u64, u64, bool)> = segments
        .iter()
        .map(|segment| (segment.start_seq.get(), segment.bytes, segment.closed_at.is_some()))
        .collect();
    assert_eq!(starts, [(0, 60, true), (60, 0, false)]);
    assert_eq!(fs::metadata(setup.file(pty, 0)).unwrap().len(), 92);
    assert_eq!(fs::metadata(setup.file(pty, 60)).unwrap().len(), 46);
    assert_eq!(range.bytes(), stream[50..70].to_vec());
    assert_eq!((range.start, range.end), (Seq::new(50), Seq::new(70)));
}

#[tokio::test]
async fn a_large_append_is_cut_into_chunks_and_segments() {
    let mut setup = setup().await;
    setup.recordings = setup.recordings.clone().with_segment_limit(150_000);
    let pty = testing::pty(1);
    let stream: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
    let mut writer = setup.recordings.start(pty).await.unwrap();

    writer.append(&stream).await.unwrap();
    let end = writer.close().await.unwrap();

    let segments = setup.segments(pty).await;
    assert_eq!(end, Seq::new(300_000));
    assert!(segments.len() >= 2, "{segments:?}");
    for segment in &segments {
        let len =
            fs::metadata(setup.dir.path().join("recordings").join(&segment.path)).unwrap().len();
        assert!(len <= 150_000, "{} is {len} bytes", segment.path.display());
    }
    let total: u64 = segments.iter().map(|segment| segment.bytes).sum();
    assert_eq!(total, 300_000);
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(range.bytes(), stream);
}

#[tokio::test]
async fn close_records_the_size_and_time_of_the_segment() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"abc").await.unwrap();
    setup.clock.advance(Duration::from_secs(3));

    let end = writer.close().await.unwrap();

    let segments = setup.segments(pty).await;
    assert_eq!(end, Seq::new(3));
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].bytes, 3);
    assert_eq!(segments[0].path, Path::new(&pty.to_string()).join("0.rec"));
    assert_eq!(segments[0].closed_at, Some(micros(setup.clock.now())));
    assert!(segments[0].started_at < micros(setup.clock.now()));
}

#[tokio::test]
async fn a_new_writer_continues_after_a_torn_tail() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"abc").await.unwrap();
    drop(writer);
    // A crash in the middle of the next chunk's header.
    let mut file = fs::OpenOptions::new().append(true).open(setup.file(pty, 0)).unwrap();
    file.write_all(b"efr\x01\x05\x00").unwrap();
    drop(file);

    let mut writer = setup.recordings.start(pty).await.unwrap();
    let seq = writer.append(b"def").await.unwrap();

    assert_eq!(seq, Seq::new(3));
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(range.bytes(), b"abcdef");
    assert_eq!(setup.segments(pty).await.len(), 1);
}

#[tokio::test]
async fn a_closed_segment_with_room_is_continued_and_opened_again() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"abc").await.unwrap();
    writer.close().await.unwrap();

    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"def").await.unwrap();

    let segments = setup.segments(pty).await;
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].closed_at, None);
    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();
    assert_eq!(range.bytes(), b"abcdef");
}

#[tokio::test]
async fn a_full_segment_is_closed_and_a_new_one_started() {
    let mut setup = setup().await;
    setup.recordings = setup.recordings.clone().with_segment_limit(40);
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(&[7; 30]).await.unwrap();
    drop(writer);

    let mut writer = setup.recordings.start(pty).await.unwrap();
    let seq = writer.append(b"next").await.unwrap();

    assert_eq!(seq, Seq::new(30));
    let starts: Vec<(u64, bool)> = setup
        .segments(pty)
        .await
        .iter()
        .map(|segment| (segment.start_seq.get(), segment.closed_at.is_some()))
        .collect();
    assert_eq!(starts, [(0, true), (30, false)]);
}

#[tokio::test]
async fn a_corrupt_segment_is_an_error_with_the_offset() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"abc").await.unwrap();
    writer.close().await.unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(setup.file(pty, 0)).unwrap();
    file.write_all(b"definitely not a chunk header").unwrap();

    let error = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap_err();

    assert!(
        matches!(error, StoreError::CorruptRecording { offset, .. } if offset == (CHUNK_HEADER_LEN + 3) as u64),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_segment_without_its_file_reads_as_empty() {
    let mut setup = setup().await;
    setup.recordings = setup.recordings.clone().with_segment_limit(40);
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(&[1; 30]).await.unwrap();
    writer.append(&[2; 30]).await.unwrap();
    fs::remove_file(setup.file(pty, 0)).unwrap();

    let range = setup.recordings.read_range(pty, Seq::ZERO, END).await.unwrap();

    assert_eq!(range.start, Seq::new(30), "the missing bytes show as a later start");
    assert_eq!(range.bytes(), vec![2; 30]);
}

#[tokio::test]
async fn files_and_directories_are_private() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();
    writer.append(b"secret output").await.unwrap();

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let file = setup.file(pty, 0);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(file.parent().unwrap()), 0o700);
    assert_eq!(mode(&setup.dir.path().join("recordings")), 0o700);
}

#[tokio::test]
async fn an_empty_append_writes_nothing() {
    let setup = setup().await;
    let pty = testing::pty(1);
    let mut writer = setup.recordings.start(pty).await.unwrap();

    let seq = writer.append(b"").await.unwrap();

    assert_eq!(seq, Seq::ZERO);
    assert_eq!(fs::metadata(setup.file(pty, 0)).unwrap().len(), 0);
}
