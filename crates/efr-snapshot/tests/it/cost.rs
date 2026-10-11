//! What snapshots cost on a fixture the size of this repository: its source files (no
//! `target/`), a fixed synthetic history of 1000 commits, and the ignored files of
//! efr's auto spec 10.2 (a build dir, `node_modules`, `.env`, small and large ignored
//! files, an `info/exclude` rule).
//!
//! The gate of 16.4 is the first snapshot of a new turn in an unchanged root: the
//! listings, the ignored walk, the `pre` commit and ref, and the mirror (the object
//! dir walk, `for-each-ref`, a fetch with no new objects). With
//! `EFR_TEST_SNAPSHOT_GATE=1` the test fails when the median CPU time of 20 such
//! snapshots (the test process and its children, so the walks in the process count)
//! is above 200 ms, or their median wall time is above a bound that grows with the
//! fixture's fastest `git status` on this machine. Without the flag it only reports.
//! `just test` and CI run it alone in a serial step with the flag; the numbers go to
//! stderr and to the CI step summary:
//! `cargo nextest run -p efr-snapshot --success-output immediate -E 'test(/^cost::/)'`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use efr_snapshot::{Limits, RootCover};
use nix::sys::resource::{UsageWho, getrusage};

use crate::support::{World, conversation, git, git_root, turn, write};

/// The default limits, as the daemon uses them.
fn limits() -> Limits {
    Limits::default()
}

/// The samples of each measure.
const SAMPLES: usize = 20;

/// The commits of the synthetic history.
const COMMITS: usize = 1000;

/// The gate of 16.4 on CPU time.
const GATE_CPU: Duration = Duration::from_millis(200);

/// The fastest `git status --porcelain` of the fixture on the machine where the gate
/// of 200 ms wall time holds as it is. A slower machine gets a bound that grows by the
/// same factor.
const REFERENCE_STATUS: Duration = Duration::from_millis(10);

/// The bound of the median wall time of a first snapshot on a machine whose fastest
/// `git status` of the fixture took `status`.
fn wall_bound(status: Duration) -> Duration {
    let factor = (status.as_secs_f64() / REFERENCE_STATUS.as_secs_f64()).max(1.0);
    GATE_CPU.mul_f64(factor)
}

#[test]
fn the_wall_bound_grows_with_a_slow_machine_only() {
    assert_eq!(wall_bound(Duration::from_millis(4)), Duration::from_millis(200));
    assert_eq!(wall_bound(Duration::from_millis(10)), Duration::from_millis(200));
    assert_eq!(wall_bound(Duration::from_millis(25)), Duration::from_millis(500));
}

/// The CPU time of this process and of its children that ended, so far.
fn cpu_now() -> Duration {
    let of = |who| {
        let usage = getrusage(who).unwrap();
        let micros = |time: nix::sys::time::TimeVal| {
            Duration::from_secs(u64::try_from(time.tv_sec()).unwrap())
                + Duration::from_micros(u64::try_from(time.tv_usec()).unwrap())
        };
        micros(usage.user_time()) + micros(usage.system_time())
    };
    of(UsageWho::RUSAGE_SELF) + of(UsageWho::RUSAGE_CHILDREN)
}

/// The CPU and wall time of `work`, and its output.
async fn measure<T>(work: impl Future<Output = T>) -> (Duration, Duration, T) {
    let (cpu, wall) = (cpu_now(), Instant::now());
    let out = work.await;
    (cpu_now().saturating_sub(cpu), wall.elapsed(), out)
}

/// Copies the source files of this repository (below `crates/`, `docs/` and `xtask/`,
/// without `target/` and hidden dirs) into `to`, with their times; the number copied.
fn copy_sources(to: &Path) -> usize {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut copied = 0;
    let mut stack: Vec<PathBuf> = ["crates", "docs", "xtask"].map(PathBuf::from).into();
    while let Some(relative) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(repo.join(&relative)) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            let child = relative.join(&*name);
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else { continue };
            if metadata.is_dir() {
                stack.push(child);
            } else if metadata.is_file() {
                let target = to.join(&child);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(entry.path(), &target).unwrap();
                // NOTE: as old as the original, like the files of a real project. A file
                // as new as the index is racy for git, which then hashes it again.
                let old = metadata.modified().unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&target)
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
                copied += 1;
            }
        }
    }
    copied
}

/// A `git fast-import` stream of [`COMMITS`] commits of `history/`, with fixed times.
fn history() -> Vec<u8> {
    let mut stream = String::new();
    for n in 1..=COMMITS {
        let time = 1_700_000_000 + n * 60;
        let message = format!("change {n}\n");
        let _ = writeln!(stream, "commit refs/heads/main\nmark :{n}");
        let _ = writeln!(stream, "author efr <efr@localhost> {time} +0000");
        let _ = writeln!(stream, "committer efr <efr@localhost> {time} +0000");
        let _ = write!(stream, "data {}\n{message}", message.len());
        if n > 1 {
            let _ = writeln!(stream, "from :{}", n - 1);
        }
        for file in 0..3 {
            let body: String =
                (0..24).map(|line| format!("commit {n} file {file} line {line}\n")).collect();
            let _ = writeln!(stream, "M 100644 inline history/{}/{}.txt", n % 50, (n + file) % 7);
            let _ = write!(stream, "data {}\n{body}\n", body.len());
        }
    }
    stream.into_bytes()
}

/// The fixture: the sources, the history, and the ignored files of 10.2.
async fn fixture(world: &World) -> (PathBuf, usize) {
    let home = world.home();
    let dir = world.dir("efr");
    let files = copy_sources(&dir);
    write(&dir.join(".gitignore"), "target/\nnode_modules/\n.env\n*.log\nbig.bin\n");
    git(&dir, &home, &[], &["init", "-q"]).await;
    let mut import = efr_stdx::process::command("git", &dir);
    import
        .args(["fast-import", "--quiet"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", &home)
        .stdin(std::process::Stdio::piped());
    let mut child = import.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&history()).await.unwrap();
    }
    assert!(child.wait().await.unwrap().success());
    git(&dir, &home, &[], &["add", "-A"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "the sources"]).await;
    for n in 0..3000 {
        write(&dir.join(format!("target/debug/deps/f{n}.o")), "o\n");
    }
    for n in 0..500 {
        write(&dir.join(format!("node_modules/pkg{}/index{n}.js", n % 20)), "m\n");
    }
    for n in 0..200 {
        write(&dir.join(format!("logs/run{n}.log")), "log line\n");
    }
    write(&dir.join(".env"), "TOKEN=1\n");
    std::fs::write(dir.join("big.bin"), vec![b'b'; 2 * 1024 * 1024]).unwrap();
    write(&dir.join(".git/info/exclude"), "local/\n");
    write(&dir.join("local/notes.md"), "n\n");
    (dir, files)
}

/// The fastest of three `git status --porcelain` runs of `dir`.
async fn fastest_status(dir: &Path, home: &Path) -> Duration {
    let mut fastest = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        git(dir, home, &[], &["status", "--porcelain"]).await;
        fastest = fastest.min(start.elapsed());
    }
    fastest
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn max(times: &[Duration]) -> Duration {
    times.iter().copied().max().unwrap_or_default()
}

/// The lines of the report, for stderr and the CI step summary.
#[derive(Default)]
struct Report(Vec<String>);

impl Report {
    fn line(&mut self, line: String) {
        self.0.push(line);
    }

    #[expect(clippy::print_stderr, reason = "the test reports what it measured")]
    fn finish(&self, gate: &str) {
        for line in &self.0 {
            eprintln!("cost: {line}");
        }
        eprintln!("cost: {gate}");
        // NOTE: GitHub's runner names the file of the step summary; the gate step
        // writes its numbers there.
        if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
            let mut text = String::from("### Snapshot cost (efr-snapshot)\n\n");
            for line in &self.0 {
                let _ = writeln!(text, "- {line}");
            }
            let _ = writeln!(text, "- **{gate}**\n");
            let mut file =
                std::fs::OpenOptions::new().append(true).create(true).open(summary).unwrap();
            std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_snapshot_of_a_turn_on_a_fixture_of_this_repository() {
    let gate = std::env::var_os("EFR_TEST_SNAPSHOT_GATE").is_some_and(|value| value == "1");
    let world = World::new();
    let home = world.home();
    let (dir, files) = fixture(&world).await;
    let roots = || vec![git_root(&dir)];
    let mut report = Report::default();

    let (cpu, wall, first) =
        measure(world.snapshots.before_call(conversation(1), turn(1), roots(), limits())).await;
    assert!(!first.is_empty());
    world.snapshots.after_call(first, limits()).await;
    let end = world.snapshots.turn_end(turn(1), limits()).await;
    assert!(end.kept && end.not_covered.is_empty(), "{end:?}");
    report.line(format!(
        "{files} source files, {COMMITS} commits; the first snapshot ever (all files hashed, the whole history fetched): wall {wall:?}, cpu {cpu:?}"
    ));

    // Two turns to warm up, then the samples, each a new turn in an unchanged root.
    let mut status = Duration::MAX;
    let (mut cpus, mut walls) = (Vec::new(), Vec::new());
    for n in 0..(SAMPLES + 2) {
        status = status.min(fastest_status(&dir, &home).await);
        let id = turn(10 + n as u128);
        let (cpu, wall, call) =
            measure(world.snapshots.before_call(conversation(1), id, roots(), limits())).await;
        assert!(!call.is_empty());
        world.snapshots.after_call(call, limits()).await;
        let end = world.snapshots.turn_end(id, limits()).await;
        assert!(end.kept && end.not_covered.is_empty(), "{end:?}");
        if n >= 2 {
            cpus.push(cpu);
            walls.push(wall);
        }
    }
    let (cpu_median, wall_median) = (median(cpus.clone()), median(walls.clone()));
    let bound = wall_bound(status);
    report.line(format!(
        "the first snapshot of a turn in an unchanged root (pre, mirror, ignored walk): cpu median {cpu_median:?} (max {:?}), wall median {wall_median:?} (max {:?})",
        max(&cpus),
        max(&walls)
    ));
    report.line(format!(
        "the fastest git status of the fixture: {status:?}; the wall bound: {bound:?}"
    ));

    // One turn after a new commit: the fetch moves the new objects.
    write(&dir.join("crates/efr-snapshot/src/new.rs"), "// new\n");
    git(&dir, &home, &[], &["add", "-A"]).await;
    git(&dir, &home, &[], &["commit", "-q", "-m", "one more"]).await;
    let id = turn(50);
    let (cpu, wall, call) =
        measure(world.snapshots.before_call(conversation(1), id, roots(), limits())).await;
    report.line(format!(
        "the first snapshot of a turn after a new commit: wall {wall:?}, cpu {cpu:?}"
    ));

    // The calls of a turn, and the coverage check of a large `rm -r` (check C2-6).
    world.snapshots.after_call(call, limits()).await;
    let mut quiet = Vec::new();
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let call = world.snapshots.before_call(conversation(1), id, roots(), limits()).await;
        let changes = world.snapshots.after_call(call, limits()).await;
        quiet.push(start.elapsed());
        assert_eq!(changes, None);
    }
    let mut changing = Vec::new();
    for n in 0..SAMPLES {
        let start = Instant::now();
        let call = world.snapshots.before_call(conversation(1), id, roots(), limits()).await;
        let before = start.elapsed();
        write(&dir.join(format!("crates/efr-snapshot/src/edit{}.rs", n % 3)), &format!("// {n}\n"));
        let start = Instant::now();
        let changes = world.snapshots.after_call(call, limits()).await;
        changing.push(before + start.elapsed());
        assert!(changes.is_some());
    }
    report.line(format!(
        "a call that changes nothing: median {:?}, max {:?}; a call that changes one file: median {:?}, max {:?}",
        median(quiet.clone()),
        max(&quiet),
        median(changing.clone()),
        max(&changing)
    ));
    let asks = [RootCover::Path(dir.join("crates"))];
    let (cpu, wall, covered) =
        measure(world.snapshots.covers(conversation(1), id, &git_root(&dir), &asks, limits()))
            .await;
    report.line(format!(
        "the coverage check of rm -r crates/: {covered:?}, wall {wall:?}, cpu {cpu:?}"
    ));
    let start = Instant::now();
    world.snapshots.turn_end(id, limits()).await;
    report.line(format!("the end of the turn: {:?}", start.elapsed()));

    // NOTE: the home store walks a dir before a write that can change all of it; the
    // walk itself is efrd's, this is what one of 2000 files costs on this machine.
    let parent = world.dir("parent");
    for n in 0..2000 {
        write(&parent.join(format!("d{}/f{n}", n % 20)), "x\n");
    }
    let start = Instant::now();
    let mut stack = vec![parent.clone()];
    let mut read = 0_usize;
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap().filter_map(Result::ok) {
            let metadata = std::fs::symlink_metadata(entry.path()).unwrap();
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                read += std::fs::read(entry.path()).unwrap().len();
            }
        }
    }
    report.line(format!(
        "a walk and read of 2000 files (the home walk of a Parent(p) bind): {:?} ({read} bytes)",
        start.elapsed()
    ));

    let passes = cpu_median <= GATE_CPU && wall_median <= bound;
    let verdict = format!(
        "gate: {} (cpu median {cpu_median:?} of at most {GATE_CPU:?}, wall median {wall_median:?} of at most {bound:?}){}",
        if passes { "passes" } else { "fails" },
        if gate { "" } else { "; report only without EFR_TEST_SNAPSHOT_GATE=1" }
    );
    report.finish(&verdict);
    if gate {
        assert!(passes, "{verdict}");
    }
    // NOTE: a loose bound, so a busy machine does not fail a run without the gate; the
    // budget of a call is 15 ms and the report shows the real numbers.
    assert!(median(changing) < Duration::from_millis(250));
}
