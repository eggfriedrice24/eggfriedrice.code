//! The bench of one pass of the recording collector (the plan's W3-A): the time of a
//! pass that keeps everything, as most hourly passes do, and of a pass that deletes
//! half the segments, over a ladder of segment counts. Ignored; `just bench-perf` runs
//! it in release.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use efr_protocol::Event;
use efr_stdx::time::Clock as _;
use efr_test_support::bench::Report;
use pretty_assertions::assert_eq;

use super::*;
use crate::{StoreConfig, sql};

/// The segment counts of the ladder.
const LADDER: [u64; 3] = [1_000, 3_000, 10_000];

/// The segments of each PTY.
const PER_PTY: u64 = 10;

/// The bytes of each segment file.
const FILE_LEN: usize = 256;

/// The passes timed at each size.
const RUNS: usize = 5;

/// The budget of the slope of a pass over the segments: the pass reads each row and
/// lists each file once, so it grows linearly. A sort or a query per PTY would bend it
/// up.
const MAX_SLOPE: f64 = 1.3;

/// The budget of one pass over the largest size, which deletes half of it: about
/// 0.1 s on a busy machine.
const MAX_PASS: Duration = Duration::from_secs(1);

/// The dir of the bench files: cargo's target temp dir, never `/tmp`. A unit test gets
/// no `CARGO_TARGET_TMPDIR`, so the dir comes from the test binary, which cargo puts at
/// `<target>/<profile>/deps/<binary>`.
fn perf_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.ancestors().nth(3).unwrap().join("tmp/perf-recordings");
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A store file and its recordings with `segments` closed segments: half of the PTYs
/// run, the other half exited a day after the clock's start.
async fn build(segments: u64) -> (TempDir, Store, Recordings, Arc<TestClock>) {
    let dir = tempfile::tempdir_in(perf_dir()).unwrap();
    let clock = TestClock::new();
    let store = Store::open(StoreConfig::in_data_dir(dir.path()), clock.clone()).await.unwrap();
    let root = dir.path().join("recordings");
    let recordings =
        Recordings::new(&root, store.writer().clone(), store.readers().clone(), clock.clone());
    let ptys = segments / PER_PTY;
    let mut batch = Batch::new();
    for n in 0..ptys {
        let pty_id = testing::pty(n);
        batch = batch
            .event(conversation(n), testing::created(None))
            .event(conversation(n), Event::ShellStarted { pty_id, cwd: "/".into(), pid: None });
    }
    store.writer().append(batch).await.unwrap();
    clock.advance(DAY);
    let mut batch = Batch::new();
    for n in (0..ptys).filter(|n| n % 2 == 1) {
        let exited = Event::ShellExited { pty_id: testing::pty(n), exit_code: Some(0) };
        batch = batch.event(conversation(n), exited);
    }
    store.writer().append(batch).await.unwrap();
    let at = sql::micros(clock.now());
    testing::on_writer(store.writer(), move |conn| {
        let mut insert = conn.prepare(
            "INSERT INTO recording_segments (pty_id, start_seq, path, bytes, started_at, \
             closed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )?;
        for n in 0..ptys {
            let pty_id = testing::pty(n);
            for k in 0..PER_PTY {
                let start = (k * FILE_LEN as u64) as i64;
                let path = sql::path_text(&relative_path(pty_id, start as u64));
                insert.execute(params![pty_id.to_string(), start, path, FILE_LEN as i64, at])?;
            }
        }
        Ok(())
    })
    .await
    .unwrap();
    for n in 0..ptys {
        let pty_id = testing::pty(n);
        crate::recording::create_dir(&root, pty_id).unwrap();
        for k in 0..PER_PTY {
            fs::write(root.join(relative_path(pty_id, k * FILE_LEN as u64)), [0; FILE_LEN])
                .unwrap();
        }
    }
    (dir, store, recordings, clock)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_recording_retention() {
    let mut report = Report::new("bench_perf_recording_retention");
    let mut keeps = Vec::new();
    let mut deletes = Vec::new();
    for segments in LADDER {
        let (_dir, store, recordings, clock) = build(segments).await;
        let label = format!("{}k segments", segments / 1_000);
        let keep = Retention::new(Some(AGE), Some(u64::MAX), 1024 * 1024);
        let mut times = Vec::new();
        for _ in 0..RUNS {
            let started = Instant::now();
            let collected = recordings.collect(keep).await.unwrap();
            times.push(started.elapsed());
            assert_eq!(collected.segments + collected.orphans, 0, "{collected:?}");
        }
        let stats = report.time(&format!("{label}: a pass that keeps all"), &times);
        keeps.push((segments, stats.p50));

        // The exited half is older than the age now.
        clock.advance(AGE + Duration::from_secs(1));
        let started = Instant::now();
        let collected = recordings.collect(keep).await.unwrap();
        let took = started.elapsed();
        report.time(&format!("{label}: a pass that deletes half"), &[took]);
        report.count(&format!("{label}: segments deleted"), collected.segments);
        assert_eq!(collected.segments, segments / 2);
        assert_eq!(collected.ptys, segments / PER_PTY / 2);
        assert_eq!(collected.freed, segments / 2 * FILE_LEN as u64);
        deletes.push((segments, took));
        drop(recordings);
        store.close().await;
    }
    let keep_slope = report.slope("a pass that keeps all over the segments", &keeps);
    let delete_slope = report.slope("a pass that deletes half over the segments", &deletes);
    report.finish();
    assert!(keep_slope < MAX_SLOPE, "a pass that keeps all grew with slope {keep_slope:.2}");
    assert!(delete_slope < MAX_SLOPE, "a pass that deletes grew with slope {delete_slope:.2}");
    let largest = deletes.last().map_or(Duration::ZERO, |(_, took)| *took);
    assert!(largest < MAX_PASS, "a pass over the largest size took {largest:?}");
}
