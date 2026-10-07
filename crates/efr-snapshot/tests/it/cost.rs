//! What the snapshots of one call cost on a copy of this repository (the tracked files,
//! no `target/`), after the first snapshot. The numbers go to stderr; run with
//! `cargo nextest run -p efr-snapshot --success-output immediate -E 'test(/^cost::/)'`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_snapshot::Limits;

use crate::support::{World, conversation, git, root, turn, write};

/// The default limits, as the daemon uses them.
fn limits() -> Limits {
    Limits::default()
}

const CALLS: usize = 20;

/// Copies the tracked files of this repository into `to`.
async fn copy_repository(home: &Path, to: &Path) -> usize {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let listed = git(&repo, home, &[], &["ls-files", "-z"]).await;
    let mut copied = 0;
    for name in listed.split('\0').filter(|name| !name.is_empty()) {
        let from = repo.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&from) else { continue };
        if !metadata.is_file() {
            continue;
        }
        let target = to.join(name);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(&from, &target).unwrap();
        // NOTE: as old as the original, like the files of a real project. A file as new
        // as the index is racy for git, which then hashes it again at every look.
        let old = metadata.modified().unwrap();
        std::fs::File::options().write(true).open(&target).unwrap().set_modified(old).unwrap();
        copied += 1;
    }
    copied
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn max(times: &[Duration]) -> Duration {
    times.iter().copied().max().unwrap_or_default()
}

#[expect(clippy::print_stderr, reason = "the test reports what it measured")]
fn report(line: &str) {
    eprintln!("cost: {line}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_snapshots_of_a_call_on_this_repository() {
    let world = World::new();
    let home = world.home();
    let dir = world.dir("efr");
    let files = copy_repository(&home, &dir).await;
    let roots = || vec![root(&dir)];

    let start = Instant::now();
    let first = world.snapshots.before_call(conversation(1), turn(1), roots(), limits()).await;
    let first_time = start.elapsed();
    assert!(!first.is_empty());
    world.snapshots.after_call(first, limits()).await;

    let mut quiet = Vec::new();
    for _ in 0..CALLS {
        let start = Instant::now();
        let call = world.snapshots.before_call(conversation(1), turn(1), roots(), limits()).await;
        let changes = world.snapshots.after_call(call, limits()).await;
        quiet.push(start.elapsed());
        assert_eq!(changes, None);
    }

    let mut changing = Vec::new();
    for n in 0..CALLS {
        let start = Instant::now();
        let call = world.snapshots.before_call(conversation(1), turn(1), roots(), limits()).await;
        let before = start.elapsed();
        write(&dir.join(format!("crates/efr-snapshot/src/edit{}.rs", n % 3)), &format!("// {n}\n"));
        let start = Instant::now();
        let changes = world.snapshots.after_call(call, limits()).await;
        changing.push(before + start.elapsed());
        assert!(changes.is_some());
    }
    let start = Instant::now();
    let turn_end = world.snapshots.finish_turn(turn(1), limits()).await;
    let end_time = start.elapsed();
    assert!(turn_end.is_some());

    report(&format!("{files} files; first snapshot {first_time:?}"));
    report(&format!(
        "a call that changes nothing: median {:?}, max {:?}",
        median(quiet.clone()),
        max(&quiet)
    ));
    report(&format!(
        "a call that changes one file: median {:?}, max {:?}",
        median(changing.clone()),
        max(&changing)
    ));
    report(&format!("the end of the turn: {end_time:?}"));
    // NOTE: a loose bound, so a busy machine does not fail the test; the budget is
    // 15 ms per call and the report shows the real numbers.
    assert!(median(changing) < Duration::from_millis(250));
}
