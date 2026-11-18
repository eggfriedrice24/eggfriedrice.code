//! The watch of a job: the batch follower on the test clock, the batch that waits at
//! the job's end on each path, and the notices of batches after a restart.

use std::io::Write as _;
use std::sync::Arc;

use efr_protocol::{Event, JobId, NoticeKind};
use efr_store::Batch;
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::{Scene, record_start};
use crate::jobs::output::{MatchWindow, Matched, Matcher, Output, spawn_reader};
use crate::jobs::{BATCH_WINDOW, OUTPUT_GRACE};

/// A batch of a `job_watch_matched` event: its first line, the other lines, the count
/// of the lines left out and the notices left.
type Recorded = (String, Vec<String>, u32, u8);

/// A batch follower of a job of the scene, over a pipe whose write end the test holds
/// as the job's output.
struct Followed {
    writer: Option<std::io::PipeWriter>,
    window: Arc<MatchWindow>,
    output: Output,
    end: CancellationToken,
    task: tokio::task::JoinHandle<Option<Matched>>,
}

impl Followed {
    /// The follower of job `number`, whose watch matches lines that start with `match`
    /// and has `wakes` notices.
    async fn new(scene: &Scene, number: u32, wakes: u8) -> Followed {
        let info = record_start(scene, scene.info(number), 0, 0).await;
        let (reader, writer) = std::io::pipe().unwrap();
        let window = Arc::new(MatchWindow::default());
        let watch = Some((Matcher::new("^match").unwrap(), Arc::clone(&window)));
        let output = spawn_reader(&format!("j{number}"), reader, None, watch).unwrap();
        let end = CancellationToken::new();
        let follow = scene.table.clone().follow_matches(
            info,
            Arc::clone(&window),
            output.clone(),
            end.clone(),
            wakes,
        );
        Followed { writer: Some(writer), window, output, end, task: tokio::spawn(follow) }
    }

    /// Writes `text` as the job's output and waits until the reader read it.
    async fn write(&mut self, text: &str) {
        let after = self.output.bytes() + text.len() as u64;
        self.writer.as_mut().unwrap().write_all(text.as_bytes()).unwrap();
        Wait::new("the reader").until(|| self.output.bytes() >= after).await.unwrap();
    }
}

/// The lines `match <from>` to `match <to>`.
fn numbered(from: u32, to: u32) -> Vec<String> {
    (from..=to).map(|n| format!("match {n}")).collect()
}

/// `lines` as the output of a job, one line each.
fn output_of(lines: &[String]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// How many batch windows the follower started.
fn window_count(scene: &Scene) -> usize {
    scene.clock.requested_sleeps().iter().filter(|sleep| **sleep == BATCH_WINDOW).count()
}

/// Waits until the follower started its `n`th batch window.
async fn window_open(scene: &Scene, n: usize) {
    Wait::new("a batch window").until(|| window_count(scene) >= n).await.unwrap();
}

/// The batches that the store recorded, in order.
async fn recorded(scene: &Scene) -> Vec<Recorded> {
    let events = scene.store.events().await.unwrap();
    events
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            Event::JobWatchMatched { line, more, dropped, wakes_left, .. } => {
                Some((line, more, dropped, wakes_left))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn lines_within_one_window_are_one_notice() {
    let scene = Scene::new().await;
    let mut followed = Followed::new(&scene, 1, 3).await;
    followed.write("match 1\nother\nmatch 2\n").await;
    window_open(&scene, 1).await;
    followed.write("match 3\n").await;
    scene.clock.advance(BATCH_WINDOW);

    let notice = scene.notice().await;
    assert_eq!((notice.input.kind, notice.input.wakes), (NoticeKind::Watch, true));
    let text = &notice.input.text;
    assert!(
        text.contains("\nmatched: match 1\nmatched: match 2\nmatched: match 3\nnotices left: 2\n"),
        "{text}"
    );
    assert_eq!(recorded(&scene).await, [("match 1".to_owned(), numbered(2, 3), 0, 2)]);
    // NOTE: the next line opens a window of its own.
    followed.write("match 4\n").await;
    window_open(&scene, 2).await;
    scene.clock.advance(BATCH_WINDOW);
    let notice = scene.notice().await;
    assert!(notice.input.text.contains("\nmatched: match 4\nnotices left: 1\n"));
    assert_eq!(recorded(&scene).await.len(), 2);
    drop(followed.writer.take());
    assert_eq!(followed.task.await.unwrap(), None, "no batch waited at the end");
}

#[tokio::test]
async fn a_burst_is_one_notice_of_its_first_and_last_ten_lines() {
    let scene = Scene::new().await;
    let mut followed = Followed::new(&scene, 1, 3).await;
    followed.write(&output_of(&numbered(1, 5_000))).await;
    window_open(&scene, 1).await;
    scene.clock.advance(BATCH_WINDOW);

    let notice = scene.notice().await;
    let mut more = numbered(2, 10);
    more.extend(numbered(4_991, 5_000));
    assert_eq!(recorded(&scene).await, [("match 1".to_owned(), more, 4_980, 2)]);
    let text = &notice.input.text;
    assert_eq!(text.matches("\nmatched: ").count(), 20, "{text}");
    assert!(text.contains("[efr: 4980 more matched lines between these are not shown"), "{text}");
    assert_eq!(window_count(&scene), 1, "a burst opens one window");
    drop(followed.writer.take());
    assert_eq!(followed.task.await.unwrap(), None);
}

#[tokio::test]
async fn max_wakes_counts_batches_and_the_last_closes_the_window() {
    let scene = Scene::new().await;
    let mut followed = Followed::new(&scene, 1, 2).await;
    for n in 1..=2 {
        followed.write(&format!("match {n}a\nmatch {n}b\n")).await;
        window_open(&scene, n).await;
        scene.clock.advance(BATCH_WINDOW);
        scene.notice().await;
    }

    let wakes: Vec<u8> = recorded(&scene).await.into_iter().map(|batch| batch.3).collect();
    assert_eq!(wakes, [1, 0], "two batches of two lines use the two notices");
    let ended = (&mut followed.task).await.unwrap();
    assert_eq!(ended, None, "the follower ends with the last notice");
    assert!(followed.window.is_closed());
    followed.write("match 3\n").await;
    assert_eq!(followed.window.take(false), None, "the reader puts no line in after the last");
    assert_eq!(recorded(&scene).await.len(), 2);
}

#[tokio::test]
async fn the_end_of_the_output_takes_the_waiting_batch() {
    let scene = Scene::new().await;
    // NOTE: the output ends in the batch window.
    let mut followed = Followed::new(&scene, 1, 3).await;
    followed.write("match 1\nmatch 2\n").await;
    window_open(&scene, 1).await;
    drop(followed.writer.take());
    let matched = followed.task.await.unwrap();
    assert_eq!(matched, Matched::of(numbered(1, 2), 0));
    // NOTE: the output ends before the follower woke for its first line.
    let mut followed = Followed::new(&scene, 2, 3).await;
    followed.write("match 3\n").await;
    drop(followed.writer.take());
    let matched = followed.task.await.unwrap();
    assert_eq!(matched, Matched::of(numbered(3, 3), 0));
    assert!(recorded(&scene).await.is_empty(), "no batch became a notice of its own");
}

#[tokio::test]
async fn a_cancel_takes_the_waiting_batch_at_once() {
    let scene = Scene::new().await;
    let mut followed = Followed::new(&scene, 1, 3).await;
    followed.write("match 1\n").await;
    window_open(&scene, 1).await;
    followed.end.cancel();
    let matched = followed.task.await.unwrap();
    assert_eq!(matched, Matched::of(numbered(1, 1), 0));
    assert!(recorded(&scene).await.is_empty());
}

/// A launcher that prints 30 matching lines and ends at once.
const PRINTS: &str = r#"#!/bin/sh
dir=$3
byte=$(dd bs=1 count=1 2> /dev/null)
if [ -z "$byte" ]; then exit 125; fi
: > "$dir/started"
i=1
while [ $i -le 30 ]; do echo "match $i"; i=$((i+1)); done
printf '{"started":true,"exit_code":0}' > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
"#;

/// A launcher that ends at once and leaves a process that holds the output: it writes
/// its pid to `$dir/holder`, prints `match late` once `$dir/go` exists and then sleeps.
/// Each wait of it ends within 30 s, so a failed test leaves no process for long.
const HOLDS: &str = r#"#!/bin/sh
dir=$3
byte=$(dd bs=1 count=1 2> /dev/null)
if [ -z "$byte" ]; then exit 125; fi
: > "$dir/started"
sh -c 'echo $$ > "$1/holder.tmp"; mv "$1/holder.tmp" "$1/holder"; i=0
while [ ! -e "$1/go" ] && [ $i -lt 300 ]; do sleep 0.1; i=$((i+1)); done
echo "match late"; exec sleep 30' sh "$dir" &
printf '{"started":true,"exit_code":0}' > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
"#;

/// Starts job 1 of `scene` with a watch of lines that start with `match`.
async fn start_watched(scene: &Scene) -> std::path::PathBuf {
    let mut job = scene.new_job(1);
    job.matcher = Some(Matcher::new("^match").unwrap());
    job.wakes = 3;
    let call_dir = job.call_dir.clone();
    let mut spawned = scene.table.spawn(&job).unwrap();
    scene.table.start(&mut spawned, &job).await.unwrap();
    scene.table.begin(spawned, job, None).await.unwrap();
    call_dir
}

#[tokio::test]
async fn a_burst_and_then_an_exit_go_into_the_end_notice() {
    let scene = Scene::with_launcher(PRINTS).await;
    start_watched(&scene).await;

    let Event::JobEnded { matched, matched_dropped, .. } = scene.ended(1).await else {
        unreachable!()
    };
    let mut kept = numbered(1, 10);
    kept.extend(numbered(21, 30));
    assert_eq!((matched, matched_dropped), (kept, 10));
    assert!(recorded(&scene).await.is_empty(), "the batch is in the end, not a notice");
    let notice = scene.notice().await;
    assert_eq!(notice.input.kind, NoticeKind::Exited);
    let text = &notice.input.text;
    assert!(text.contains("\nmatched before the end:\nmatched: match 1\n"), "{text}");
    assert!(text.contains("\n[efr: 10 more matched lines between these"), "{text}");
}

#[tokio::test]
async fn output_that_outlives_the_grace_still_gives_its_batch_to_the_end() {
    let scene = Scene::with_launcher(HOLDS).await;
    let call_dir = start_watched(&scene).await;
    let holder = Wait::new("the holder's pid")
        .until_some(|| std::fs::read_to_string(call_dir.join("holder")).ok())
        .await
        .unwrap();
    let holder = rustix::process::Pid::from_raw(holder.trim().parse().unwrap()).unwrap();
    // NOTE: the launcher ended; the end waits for the rest of the output.
    Wait::new("the output grace")
        .until(|| scene.clock.requested_sleeps().contains(&OUTPUT_GRACE))
        .await
        .unwrap();
    // NOTE: the line comes when three quarters of the grace passed, so its batch
    // window ends after the grace: the end must take the batch.
    assert!(OUTPUT_GRACE / 4 < BATCH_WINDOW);
    scene.clock.advance(OUTPUT_GRACE / 4 * 3);
    std::fs::write(call_dir.join("go"), "").unwrap();
    let key = scene.key(1);
    let read =
        || scene.table.lock().running.get(&key).is_some_and(|entry| entry.output.bytes() > 0);
    Wait::new("the late line").until(read).await.unwrap();
    window_open(&scene, 1).await;
    scene.clock.advance(OUTPUT_GRACE / 4);

    let ended = scene.ended(1).await;
    let _ = rustix::process::kill_process(holder, rustix::process::Signal::KILL);
    let Event::JobEnded { matched, matched_dropped, output_bytes, .. } = ended else {
        unreachable!()
    };
    assert_eq!((matched, matched_dropped), (vec!["match late".to_owned()], 0));
    assert_eq!(output_bytes, "match late\n".len() as u64);
    assert!(recorded(&scene).await.is_empty(), "no notice of its own");
}

#[tokio::test]
async fn a_batch_notice_after_a_restart_has_its_lines() {
    let scene = Scene::new().await;
    let info = record_start(&scene, scene.info(1), 0, 0).await;
    let mut more = numbered(2, 10);
    more.extend(numbered(91, 100));
    let event = Event::JobWatchMatched {
        job_id: JobId::nth(1),
        line: "match 1".to_owned(),
        more,
        dropped: 80,
        wakes_left: 0,
    };
    scene.store.writer().append(Batch::new().event(info.conversation_id, event)).await.unwrap();

    let notices = scene.table.undelivered().await.unwrap();
    let [notice] = notices.try_into().unwrap();
    assert_eq!((notice.input.kind, notice.input.wakes), (NoticeKind::Watch, false));
    let text = &notice.input.text;
    assert!(text.contains("\nmatched: match 1\n"), "{text}");
    assert!(text.contains("matched: match 10\n[efr: 80 more matched lines between"), "{text}");
    assert!(text.contains("matched: match 100\nnotices left: 0\n[efr: the watch is over."));
}
