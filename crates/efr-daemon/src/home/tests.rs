use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use efr_conversation::FileBackup;
use efr_permissions::{CoverAsk, Coverage};
use efr_protocol::{ConversationId, TurnId, UndoReason};
use efr_scope::{Git, Home};
use efr_snapshot::{
    DEFAULT_SNAPSHOT_TIMEOUT, HomeDelta, HomeEntry, HomeState, SnapshotParts, Snapshots,
};
use efr_stdx::rng::Rng as _;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use crate::home::{
    HomeLimits, Prepared, Reader, Target, after, changed, collect_trash, coverage, prepare,
    root_holds,
};

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

const LIMITS: HomeLimits = HomeLimits { max_files: 2000, max_bytes: 256 * 1024 * 1024 };

fn file(path: &Path) -> Target {
    Target { ask: CoverAsk::Home(path.to_path_buf()), floor: false, link: None }
}

fn floor(path: &Path) -> Target {
    Target { ask: CoverAsk::Home(path.to_path_buf()), floor: true, link: None }
}

fn dir(path: &Path) -> Target {
    Target { ask: CoverAsk::HomeDir(path.to_path_buf()), floor: false, link: None }
}

/// The state of each entry of `prepared`, by path.
fn states(prepared: &Prepared) -> Vec<(PathBuf, HomeState)> {
    prepared.entries.iter().map(|entry| (entry.path.clone(), entry.state.clone())).collect()
}

/// The mode of each file that [`write`] makes.
const MODE: u32 = 0o644;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(MODE)).unwrap();
}

fn store(root: &Path) -> Snapshots {
    let clock = TestClock::new();
    Snapshots::new(SnapshotParts {
        dir: root.join("data/snapshots"),
        git: Git::new(clock.shared()).isolated(),
        home: Home::new(root.join("home")).unwrap(),
        clock: clock.shared(),
        excludes_file: None,
        timeout: DEFAULT_SNAPSHOT_TIMEOUT,
    })
}

fn ids() -> (ConversationId, TurnId) {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    (ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2)))
}

#[test]
fn home_copy_records_missing_path() {
    let (_temp, root) = root();
    let conf = root.join("home/.config/hypr/monitors.conf");

    let prepared = prepare(&[floor(&conf)], &[], LIMITS).unwrap();

    assert_eq!(states(&prepared), [(conf, HomeState::Missing)]);
    assert!(prepared.not_covered.is_empty());
    assert!(prepared.covers(&CoverAsk::Home(root.join("home/.config/hypr/monitors.conf"))));
}

#[test]
fn home_copy_never_follows_symlink() {
    let (_temp, root) = root();
    let home = root.join("home");
    let real = home.join("dotfiles/zsh/.zshrc");
    write(&real, "old\n");
    std::os::unix::fs::symlink(&real, home.join(".zshrc")).unwrap();
    let outside = root.join("outside");
    write(&outside.join("secret.txt"), "not mine to read\n");
    std::fs::create_dir_all(home.join("notes")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("notes/elsewhere")).unwrap();

    let linked = prepare(&[floor(&home.join(".zshrc"))], &[], LIMITS).unwrap();
    let walked = prepare(&[dir(&home.join("notes"))], &[], LIMITS).unwrap();

    assert_eq!(
        states(&linked),
        [
            (
                home.join(".zshrc"),
                HomeState::Link { target: real.as_os_str().as_encoded_bytes().to_vec() }
            ),
            (real.clone(), HomeState::File { mode: MODE }),
        ]
    );
    assert_eq!(linked.entries[1].chain, [home.join(".zshrc")], "the links on the way");
    assert_eq!(linked.shown, [(real, home.join(".zshrc"))]);
    let target = outside.as_os_str().as_encoded_bytes().to_vec();
    assert_eq!(
        states(&walked),
        [(home.join("notes/elsewhere"), HomeState::Link { target })],
        "a link in a walked dir stays a link, and the walk never goes through it"
    );
}

#[test]
fn home_copy_skips_masks_and_secrets() {
    let (_temp, root) = root();
    let home = root.join("home");
    write(&home.join(".aws/config"), "[default]\n");
    write(&home.join("work/notes.md"), "notes\n");
    write(&home.join("work/.ssh/id_ed25519"), "KEY\n");
    let masks = [home.join(".aws"), home.join("work/.ssh")];

    let named = prepare(&[file(&home.join(".aws/config"))], &masks, LIMITS).unwrap();
    let walked = prepare(&[dir(&home.join("work"))], &masks, LIMITS).unwrap();
    let floor = prepare(&[floor(&home.join(".aws/config"))], &masks, LIMITS);

    assert_eq!(
        states(&named),
        [(home.join(".aws/config"), HomeState::NotCovered(UndoReason::Masked))],
        "a masked file runs with no copy"
    );
    let mut found = states(&walked);
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        found,
        [
            (home.join("work/.ssh"), HomeState::NotCovered(UndoReason::Masked)),
            (home.join("work/notes.md"), HomeState::File { mode: MODE }),
        ],
        "the walk never goes into a mask"
    );
    assert!(!walked.covers(&CoverAsk::HomeDir(home.join("work"))));
    assert!(
        floor.unwrap_err().contains("secret or a masked file"),
        "a masked floor stops the call"
    );
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("work")), &masks, LIMITS), Coverage::Masked);
    assert_eq!(
        coverage(&CoverAsk::Home(home.join(".aws/config")), &masks, LIMITS),
        Coverage::Masked
    );
}

#[test]
fn home_copy_dir_over_limit_says_not_covered() {
    let (_temp, root) = root();
    let home = root.join("home");
    for n in 0..3 {
        write(&home.join(format!("big/{n}.txt")), "x\n");
    }
    let small = HomeLimits { max_files: 2, max_bytes: 1024 };
    let bytes = HomeLimits { max_files: 10, max_bytes: 4 };

    let prepared = prepare(&[dir(&home.join("big"))], &[], small).unwrap();

    assert_eq!(
        states(&prepared),
        [(home.join("big"), HomeState::NotCovered(UndoReason::OverLimit))]
    );
    assert_eq!(prepared.not_covered, [(home.join("big"), UndoReason::OverLimit)]);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], small), Coverage::OverLimit);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], bytes), Coverage::OverLimit);
    assert_eq!(coverage(&CoverAsk::HomeDir(home.join("big")), &[], LIMITS), Coverage::Covered);
}

#[tokio::test]
async fn home_copy_of_a_parent_bind_walks_the_dir() {
    let (_temp, root) = root();
    let home = root.join("home");
    let conf = home.join(".config/app/app.conf");
    write(&conf, "a=1\n");
    write(&home.join(".config/app/theme.conf"), "dark\n");
    let parent = home.join(".config/app");
    let targets = [file(&conf), dir(&parent)];
    let store = store(&root);
    let (conversation, turn) = ids();

    let before = prepare(&targets, &[], LIMITS).unwrap();
    let copy = store.home_before(conversation, turn, Reader::new(&before)).await.unwrap();
    write(&conf, "a=2\n");
    write(&parent.join("new.conf"), "made by the call\n");
    let after_copy = after(&before, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&after_copy)).await.unwrap();
    let backups = changed(&deltas, &before, &after_copy);

    assert_eq!(before.walked, std::slice::from_ref(&parent));
    assert_eq!(before.entries.len(), 2, "the target and the other file of its dir");
    let paths: Vec<(&Path, bool)> =
        backups.iter().map(|backup| (backup.path.as_path(), backup.reason.is_none())).collect();
    assert_eq!(paths, [(conf.as_path(), true), (parent.join("new.conf").as_path(), true)]);
}

#[test]
fn home_copy_of_a_large_parent_stops_at_the_limit() {
    let (_temp, root) = root();
    let home = root.join("home");
    let conf = home.join("app/app.conf");
    write(&conf, "a=1\n");
    for n in 0..5 {
        write(&home.join(format!("app/data/{n}")), "x\n");
    }
    let small = HomeLimits { max_files: 3, max_bytes: 1024 * 1024 };

    let prepared = prepare(&[file(&conf), dir(&home.join("app"))], &[], small).unwrap();

    assert_eq!(
        states(&prepared),
        [
            (conf.clone(), HomeState::File { mode: MODE }),
            (home.join("app"), HomeState::NotCovered(UndoReason::OverLimit)),
        ],
        "the target alone, and the dir is not covered"
    );
    assert!(prepared.walked.is_empty());
    assert!(prepared.covers(&CoverAsk::Home(conf)));
    assert!(!prepared.covers(&CoverAsk::HomeDir(home.join("app"))));
}

#[test]
fn home_copy_failure_of_a_floor_stops_the_call() {
    let (_temp, root) = root();
    let home = root.join("home");
    let big = home.join(".zshrc");
    std::fs::create_dir_all(&home).unwrap();
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(efr_snapshot::MAX_HOME_FILE_BYTES + 1).unwrap();
    let notes = home.join("notes.bin");
    std::fs::copy(&big, &notes).unwrap();

    let floor = prepare(&[floor(&big)], &[], LIMITS);
    let other = prepare(&[self::file(&notes)], &[], LIMITS).unwrap();

    let reason = floor.unwrap_err();
    assert!(reason.contains("larger than 16 MiB") && reason.contains("did not run"), "{reason}");
    assert_eq!(states(&other), [(notes, HomeState::NotCovered(UndoReason::OverLimit))]);
}

#[test]
fn old_undo_trash_is_collected() {
    let (_temp, root) = root();
    let trash = root.join("undo-trash");
    let clock = TestClock::new();
    let rng = TestRng::new(3);
    let day = Duration::from_secs(24 * 3600);
    let old = format!("u-{}", efr_stdx::id::uuid_v7(&clock, &rng).hyphenated());
    clock.advance(10 * day);
    let new = format!("u-{}", efr_stdx::id::uuid_v7(&clock, &rng).hyphenated());
    for name in [&old, &new, &"kept".to_owned()] {
        write(&trash.join(name).join("home/x"), "x\n");
    }
    let now = SystemTime::from(efr_stdx::time::Clock::now(&clock));

    let removed = collect_trash(&trash, 5 * day, now);

    assert_eq!(removed, 1);
    assert!(!trash.join(&old).exists() && trash.join(&new).exists());
    assert!(trash.join("kept").exists(), "a name that is not an undo stays");
}

#[tokio::test]
async fn home_copy_names_an_uncovered_file_of_a_walked_dir_only_when_it_changed() {
    let (_temp, root) = root();
    let home = root.join("home");
    let docs = home.join("Documents");
    write(&docs.join("notes.md"), "notes\n");
    let big = docs.join("video.mkv");
    std::fs::File::create(&big).unwrap().set_len(20 * 1024 * 1024).unwrap();
    let targets = [file(&docs.join("notes.md")), dir(&docs)];
    let store = store(&root);
    let (conversation, turn) = ids();

    let before = prepare(&targets, &[], LIMITS).unwrap();
    let copy = store.home_before(conversation, turn, Reader::new(&before)).await.unwrap();
    write(&docs.join("notes.md"), "changed\n");
    let after_copy = after(&before, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&after_copy)).await.unwrap();
    let backups = changed(&deltas, &before, &after_copy);

    let paths: Vec<&Path> = backups.iter().map(|backup| backup.path.as_path()).collect();
    assert_eq!(paths, [docs.join("notes.md").as_path()], "the untouched large file stays quiet");

    // The call grows the large file: no copy covers it, and the turn says so.
    let copy = store.home_before(conversation, turn, Reader::new(&after_copy)).await.unwrap();
    std::fs::OpenOptions::new().append(true).open(&big).unwrap().set_len(21 * 1024 * 1024).unwrap();
    let grown = after(&after_copy, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&grown)).await.unwrap();
    let backups = changed(&deltas, &after_copy, &grown);
    let reasons: Vec<(&Path, Option<UndoReason>)> =
        backups.iter().map(|backup| (backup.path.as_path(), backup.reason)).collect();
    assert_eq!(reasons, [(big.as_path(), Some(UndoReason::OverLimit))]);
}

#[test]
fn a_masked_path_inside_a_snapshot_root_needs_no_home_copy() {
    let roots = [PathBuf::from("/home/u/p/app"), PathBuf::from("/home/u/.local/share/efr/scratch")];
    let masks = [PathBuf::from("/home/u/p/app/.env"), PathBuf::from("/home/u/.aws")];
    let home = |path: &str| CoverAsk::Home(PathBuf::from(path));

    assert!(root_holds(&home("/home/u/p/app/.env"), &roots, &masks));
    assert!(!root_holds(&home("/home/u/p/app/.envrc"), &roots, &masks), "a floor keeps its copy");
    assert!(!root_holds(&home("/home/u/.aws/config"), &roots, &masks), "outside the roots");
    assert!(!root_holds(&CoverAsk::CronTable, &roots, &masks));
}

/// The target dir's `tmp` dir, from the path of this test binary
/// (`<target>/<profile>/deps/<binary>`): a big fixture goes to the disk, not to `/tmp`.
fn target_tmp() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.ancestors().nth(3).unwrap_or(Path::new(".")).join("tmp")
}

/// The time of `f`, and its value.
async fn timed<T>(f: impl Future<Output = T>) -> (Duration, T) {
    let start = std::time::Instant::now();
    let value = f.await;
    (start.elapsed(), value)
}

/// The home copy of a directory of 200, 2,000 and 20,000 files (plan slice W2-G):
/// `prepare` before the call, the store's copy before it, `after` once the call
/// changed 10 files and made 10, the store's deltas (`home_after`) and `changed`.
/// The limit of the test config is raised above the largest size. Its budget: prepare
/// and after, and the store's deltas, grow with a slope below 1.3, and prepare and
/// after of 20,000 files take less than 1 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_home_copy() {
    use efr_test_support::bench::Report;
    let mut report = Report::new("bench_perf_home_copy");
    let limits = HomeLimits { max_files: 100_000, max_bytes: 16 * 1024 * 1024 * 1024 };
    let parent = target_tmp().join("efr-daemon-bench");
    std::fs::create_dir_all(&parent).unwrap();
    let (mut points, mut store_points) = (Vec::new(), Vec::new());
    for files in [200_usize, 2_000, 20_000] {
        let temp = tempfile::tempdir_in(&parent).unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let data = root.join("home/data");
        for n in 0..files {
            write(&data.join(format!("d{}/f{n}.txt", n / 100)), &format!("line {n}\n"));
        }
        let targets = [dir(&data)];
        let store = store(&root);
        let (conversation, turn) = ids();
        let label = format!("{files} files");

        let (prepare_time, before) = timed(async { prepare(&targets, &[], limits).unwrap() }).await;
        assert!(before.covers(&CoverAsk::HomeDir(data.clone())), "{label}: within the limits");
        let (copy_time, copy) =
            timed(store.home_before(conversation, turn, Reader::new(&before))).await;
        let copy = copy.unwrap();
        for n in 0..10 {
            write(&data.join(format!("d0/f{n}.txt")), "changed by the call\n");
            write(&data.join(format!("new/n{n}.txt")), "made by the call\n");
        }
        let (after_time, after_copy) = timed(async { after(&before, &[], limits) }).await;
        let (deltas_time, deltas) = timed(store.home_after(&copy, Reader::new(&after_copy))).await;
        let deltas = deltas.unwrap();
        let (changed_time, backups) = timed(async { changed(&deltas, &before, &after_copy) }).await;
        assert!(backups.len() >= 20, "{label}: the 10 changed and the 10 new files: {backups:?}");

        report.time(&format!("{label} prepare"), &[prepare_time]);
        report.time(&format!("{label} home_before (store copy)"), &[copy_time]);
        report.time(&format!("{label} after"), &[after_time]);
        report.time(&format!("{label} home_after (deltas)"), &[deltas_time]);
        report.time(&format!("{label} changed"), &[changed_time]);
        report.count(&format!("{label} entries before"), before.entries.len() as u64);
        report.count(&format!("{label} entries after"), after_copy.entries.len() as u64);
        report.count(&format!("{label} deltas"), deltas.len() as u64);
        report.count(&format!("{label} changed paths"), backups.len() as u64);
        points.push((files as u64, prepare_time + after_time));
        store_points.push((files as u64, deltas_time));
        let slowest = [prepare_time, copy_time, after_time, deltas_time, changed_time];
        if slowest.iter().any(|time| *time > Duration::from_secs(10)) {
            report.count("stopped_at_n", files as u64);
            break;
        }
    }
    let slope = report.slope("prepare and after slope", &points);
    let store_slope = report.slope("home_after (deltas) slope", &store_points);
    report.finish();
    // The budget (plan W2-G): with a set beside each list and an index of each
    // manifest, a copy costs O(files). The linear searches of before gave slopes of
    // 1.86 and 1.44, and 6 s for each of prepare and after at 20,000 files.
    assert_eq!(points.len(), 3, "the ladder stopped early: {points:?}");
    assert!(slope < 1.3, "prepare and after grow faster than linear: slope {slope:.2}");
    assert!(store_slope < 1.3, "the store's deltas grow faster than linear: {store_slope:.2}");
    let (most, time) = points[points.len() - 1];
    assert!(time < Duration::from_secs(1), "prepare and after of {most} files took {time:?}");
}

/// The old `Prepared::push`, with a linear search of each list: the reference of the
/// order and of the first-wins rule.
fn old_push(prepared: &mut Prepared, entry: HomeEntry) {
    if let HomeState::NotCovered(reason) = entry.state
        && !prepared.not_covered.iter().any(|(path, _)| *path == entry.path)
    {
        prepared.not_covered.push((entry.path.clone(), reason));
    }
    if !prepared.entries.iter().any(|known| known.path == entry.path) {
        prepared.entries.push(entry);
    }
}

/// The old `Prepared::push_state`.
fn old_push_state(prepared: &mut Prepared, entry: HomeEntry, bytes: Option<Vec<u8>>) {
    if let Some(bytes) = bytes {
        prepared.states.retain(|(path, _)| *path != entry.path);
        prepared.states.push((entry.path.clone(), bytes));
    }
    prepared.entries.retain(|known| known.path != entry.path);
    old_push(prepared, entry);
}

/// The old `changed`, with a linear search of each list for each delta.
fn old_changed(deltas: &[HomeDelta], before: &Prepared, after: &Prepared) -> Vec<FileBackup> {
    let reason = |path: &Path| {
        before
            .not_covered
            .iter()
            .chain(&after.not_covered)
            .find(|(known, _)| known == path)
            .map(|(_, reason)| *reason)
            .unwrap_or(UndoReason::Unknown)
    };
    let stamp = |prepared: &Prepared, path: &Path| {
        prepared.stamps.iter().find(|(known, _)| known == path).map(|(_, stamp)| *stamp)
    };
    let unchanged = |delta: &HomeDelta| {
        !delta.covered
            && stamp(before, &delta.path).is_some()
            && stamp(before, &delta.path) == stamp(after, &delta.path)
    };
    deltas
        .iter()
        .filter(|delta| delta.changed && !unchanged(delta))
        .map(|delta| {
            let link =
                before.shown.iter().find(|(real, _)| *real == delta.path).map(|(_, link)| link);
            FileBackup {
                path: link.cloned().unwrap_or_else(|| delta.path.clone()),
                backup: PathBuf::new(),
                floor: delta.floor,
                real: link.map(|_| delta.path.clone()),
                reason: (!delta.covered).then(|| reason(&delta.path)),
            }
        })
        .collect()
}

/// A number below `below` from `rng`.
fn pick(rng: &TestRng, below: u64) -> u64 {
    rng.next_u64() % below
}

/// One of a few paths, so the same path comes often.
fn some_path(rng: &TestRng) -> PathBuf {
    PathBuf::from(format!("/home/u/d{}/f{}", pick(rng, 2), pick(rng, 5)))
}

/// One of a few reasons.
fn some_reason(rng: &TestRng) -> UndoReason {
    let reasons =
        [UndoReason::Masked, UndoReason::OverLimit, UndoReason::LeftOut, UndoReason::Unknown];
    reasons[usize::try_from(pick(rng, 4)).unwrap()]
}

/// An entry of a random path and state.
fn some_entry(rng: &TestRng) -> HomeEntry {
    let state = match pick(rng, 4) {
        0 => HomeState::File { mode: 0o644 },
        1 => HomeState::Missing,
        2 => HomeState::Link { target: b"/elsewhere".to_vec() },
        _ => HomeState::NotCovered(some_reason(rng)),
    };
    HomeEntry::new(some_path(rng), state, pick(rng, 2) == 0)
}

/// The lists of `prepared` that the dedup keeps.
type Lists = (Vec<HomeEntry>, Vec<(PathBuf, UndoReason)>, Vec<PathBuf>);

fn lists(prepared: &Prepared) -> Lists {
    let states = prepared.states.iter().map(|(path, _)| path.clone()).collect();
    (prepared.entries.clone(), prepared.not_covered.clone(), states)
}

/// The paths of `paths`, as a set.
fn path_set<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> HashSet<PathBuf> {
    paths.cloned().collect()
}

#[test]
fn a_copy_keeps_the_first_entry_of_each_path_in_the_old_order() {
    let rng = TestRng::new(11);
    for _ in 0..300 {
        let (mut old, mut new) = (Prepared::default(), Prepared::default());
        for _ in 0..pick(&rng, 40) {
            if pick(&rng, 6) == 0 {
                let path =
                    if pick(&rng, 2) == 0 { PathBuf::from("@crontab") } else { some_path(&rng) };
                let entry = HomeEntry::new(path, HomeState::State, false);
                let bytes = (pick(&rng, 2) == 0).then(|| b"state\n".to_vec());
                old_push_state(&mut old, entry.clone(), bytes.clone());
                new.push_state(entry, bytes);
            } else {
                let entry = some_entry(&rng);
                old_push(&mut old, entry.clone());
                new.push(entry);
            }
            let path = some_path(&rng);
            let stamp = (pick(&rng, 3), 0, 0, 0, 0, 0);
            if !old.stamps.iter().any(|(known, _)| *known == path) {
                old.stamps.push((path.clone(), stamp));
            }
            new.push_stamp(&path, stamp);
        }
        assert_eq!(lists(&new), lists(&old));
        assert_eq!(new.stamps, old.stamps);
        assert_eq!(new.seen.entries, path_set(new.entries.iter().map(|entry| &entry.path)));
        assert_eq!(new.seen.not_covered, path_set(new.not_covered.iter().map(|(path, _)| path)));
        assert_eq!(new.seen.stamps, path_set(new.stamps.iter().map(|(path, _)| path)));
    }
}

#[test]
fn the_changed_paths_of_a_copy_are_those_of_the_old_search() {
    let rng = TestRng::new(13);
    let some_prepared = |rng: &TestRng| {
        let mut prepared = Prepared::default();
        for _ in 0..pick(rng, 12) {
            prepared.not_covered.push((some_path(rng), some_reason(rng)));
            prepared.stamps.push((some_path(rng), (pick(rng, 2), 0, 0, 0, 0, 0)));
            if pick(rng, 3) == 0 {
                prepared.shown.push((some_path(rng), some_path(rng)));
            }
        }
        prepared
    };
    for _ in 0..500 {
        let (before, after) = (some_prepared(&rng), some_prepared(&rng));
        let deltas: Vec<HomeDelta> = (0..pick(&rng, 15))
            .map(|_| HomeDelta {
                path: some_path(&rng),
                floor: pick(&rng, 2) == 0,
                changed: pick(&rng, 4) != 0,
                covered: pick(&rng, 2) == 0,
            })
            .collect();
        assert_eq!(changed(&deltas, &before, &after), old_changed(&deltas, &before, &after));
    }
}
