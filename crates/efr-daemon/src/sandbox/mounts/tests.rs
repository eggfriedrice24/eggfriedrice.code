//! The snapshot of the host's mounts and the mount namespace check. The table is a
//! file of the test, and the helper is a script that answers from a file, or answers
//! the root and then blocks; the clock is a `TestClock`, so no test waits for the
//! deadline in real time.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_sandbox::{MountSizes, MountSnapshot};
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;

use super::*;

/// The helper: it notes each run and what it was asked, prints `answers`, and with a
/// `block` file it then notes its pid and waits until it is killed.
const HELPER: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo run >> "$dir/runs"
tr '\0' '\n' > "$dir/asked"
cat "$dir/answers"
if [ -e "$dir/block" ]; then
  echo $$ > "$dir/pid"
  exec sleep 600
fi
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    clock: TestClock,
    uid: u32,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap();
        let helper = dir.join("helper");
        std::fs::write(&helper, HELPER).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("answers"), "").unwrap();
        let uid = rustix::process::getuid().as_raw();
        Fixture { _dir: temp, dir, clock: TestClock::new(), uid }
    }

    /// A table of the test's user: the root, `/tmp`, the dummy types, the user's own
    /// runtime dir with a FUSE mount below it, another user's runtime dir and FUSE
    /// mount, and a slow network mount.
    fn table(&self) -> String {
        let (me, other) = (self.uid, self.uid + 1);
        format!(
            "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n\
             2 1 0:20 / /tmp rw - tmpfs tmpfs rw\n\
             3 1 0:21 / /proc rw - proc proc rw\n\
             4 1 0:22 / /sys rw - sysfs sysfs rw\n\
             5 3 0:23 / /proc/sys/fs/binfmt_misc rw - autofs systemd-1 rw\n\
             6 1 0:24 / /run/user/{me} rw - tmpfs tmpfs rw\n\
             7 6 0:25 / /run/user/{me}/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw,user_id={me}\n\
             8 1 0:26 / /run/user/{other} rw - tmpfs tmpfs rw\n\
             9 1 0:27 / /mnt/theirs rw - fuse.sshfs h:/x rw,user_id={other}\n\
             10 1 0:28 / /mnt/slow rw - nfs4 nas:/vol rw\n"
        )
    }

    fn view(&self, reap: Option<Reap>) -> MountView {
        std::fs::write(self.dir.join("table"), self.table()).unwrap();
        MountView::with_parts(
            self.dir.join("helper"),
            self.clock.shared(),
            self.dir.join("table"),
            reap.unwrap_or_else(reap_by_waiting),
        )
    }

    /// Answers each of `paths` with sizes; the others get no answer.
    fn answer(&self, paths: &[&str]) {
        let lines: String = paths
            .iter()
            .map(|path| {
                format!(
                    "{{\"path\":\"{path}\",\"ok\":{{\"blocks\":100,\"bfree\":40,\"bavail\":30,\"frsize\":4096,\"files\":10,\"ffree\":5}}}}\n"
                )
            })
            .collect();
        std::fs::write(self.dir.join("answers"), lines).unwrap();
    }

    fn block(&self) {
        std::fs::write(self.dir.join("block"), "").unwrap();
        let _ = std::fs::remove_file(self.dir.join("pid"));
    }

    fn unblock(&self) {
        let _ = std::fs::remove_file(self.dir.join("block"));
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.join("runs")).unwrap_or_default().lines().count()
    }

    fn asked(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.join("asked")).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }

    /// The pid of the blocking helper, once it wrote it.
    async fn helper_pid(&self) -> u32 {
        Wait::new("the blocking helper's pid")
            .until_some(|| std::fs::read_to_string(self.dir.join("pid")).ok()?.trim().parse().ok())
            .await
            .unwrap()
    }
}

const MEASURED: MountSizes =
    MountSizes::Measured { blocks: 100, bfree: 40, bavail: 30, frsize: 4096, files: 10, ffree: 5 };

/// Takes a snapshot whose helper blocks: waits until the helper runs and the deadline
/// is set, then moves the clock past it.
async fn snapshot_past_deadline(fixture: &Fixture, view: &MountView) -> Arc<Taken> {
    let task = {
        let view = view.clone();
        tokio::spawn(async move { view.snapshot().await })
    };
    fixture.helper_pid().await;
    Wait::new("the deadline").until(|| fixture.clock.pending_sleeps() > 0).await.unwrap();
    fixture.clock.advance(DEADLINE);
    task.await.unwrap().unwrap()
}

#[tokio::test]
async fn mount_view_snapshot_skips_dummy_types() {
    let fixture = Fixture::new();
    fixture.answer(&["/", "/tmp", "/mnt/slow"]);
    let view = fixture.view(None);
    let taken = view.snapshot().await.unwrap();
    let me = format!("/run/user/{}", fixture.uid);
    let gvfs = format!("{me}/gvfs");
    // The helper is never asked for a dummy type, an automount point included.
    assert_eq!(fixture.asked(), ["/", "/tmp", me.as_str(), gvfs.as_str(), "/mnt/slow"]);
    for id in [3, 4, 5] {
        assert_eq!(taken.sizes(id), Some(MountSizes::Dummy), "{id}");
    }
    assert_eq!(taken.sizes(1), Some(MEASURED));
    // A mount that the helper did not answer before it ended has no sizes.
    assert_eq!(taken.sizes(6), Some(MountSizes::Failed { errno: 0 }));
}

#[tokio::test]
async fn mount_view_other_users_rows_have_no_sizes() {
    let fixture = Fixture::new();
    let other = format!("/run/user/{}", fixture.uid + 1);
    fixture.answer(&["/", other.as_str(), "/mnt/theirs"]);
    let taken = fixture.view(None).snapshot().await.unwrap();
    assert!(!fixture.asked().iter().any(|path| *path == other || path == "/mnt/theirs"));
    assert_eq!(taken.sizes(8), Some(MountSizes::OtherUser));
    assert_eq!(taken.sizes(9), Some(MountSizes::OtherUser));
}

#[tokio::test]
async fn mount_view_masked_rows_have_no_sizes() {
    let fixture = Fixture::new();
    let me = format!("/run/user/{}", fixture.uid);
    let gvfs = format!("{me}/gvfs");
    fixture.answer(&["/", "/tmp", me.as_str(), gvfs.as_str(), "/mnt/slow"]);
    let taken = fixture.view(None).snapshot().await.unwrap();
    let sizes_of = |masks: &[PathBuf], below: &[PathBuf]| {
        let (table, json) = taken.files(masks, below).unwrap();
        assert_eq!(table, fixture.table().into_bytes(), "the table goes in verbatim");
        MountSnapshot::from_json(&json).unwrap()
    };
    // A mask on the slow mount, and the runtime dir that the sandbox replaces.
    let sizes = sizes_of(&[PathBuf::from("/mnt/slow")], &[PathBuf::from(&me)]);
    assert_eq!(sizes.sizes(10), Some(&MountSizes::Masked));
    assert_eq!(sizes.sizes(7), Some(&MountSizes::Masked), "below the runtime dir");
    assert_eq!(sizes.sizes(6), Some(&MEASURED), "the runtime dir itself keeps its sizes");
    assert_eq!(sizes.sizes(3), Some(&MountSizes::Dummy));
    // With an Unmask grant, the mask is not in the call's list, and the sizes stay.
    let sizes = sizes_of(&[], &[PathBuf::from(&me)]);
    assert_eq!(sizes.sizes(10), Some(&MEASURED));
}

#[tokio::test]
async fn mount_view_statvfs_deadline_kills_helper() {
    let fixture = Fixture::new();
    fixture.answer(&["/"]);
    fixture.block();
    let view = fixture.view(None);
    let task = {
        let view = view.clone();
        tokio::spawn(async move { view.snapshot().await })
    };
    let pid = fixture.helper_pid().await;
    Wait::new("the deadline").until(|| fixture.clock.pending_sleeps() > 0).await.unwrap();
    fixture.clock.advance(DEADLINE);
    let taken = task.await.unwrap().unwrap();
    assert_eq!(taken.sizes(1), Some(MEASURED), "an answer before the deadline counts");
    assert_eq!(taken.sizes(2), Some(MountSizes::Timeout));
    assert_eq!(taken.sizes(10), Some(MountSizes::Timeout));
    // efrd killed the helper, and the reaper waited for it.
    let proc = PathBuf::from(format!("/proc/{pid}"));
    Wait::new("the helper to end").until(|| !proc.exists()).await.unwrap();
    Wait::new("the reaper").until(|| !view.helper_lives()).await.unwrap();
}

#[tokio::test]
async fn mount_view_snapshot_returns_while_helper_unreaped() {
    let fixture = Fixture::new();
    fixture.block();
    // A reaper that never ends, as one that waits for a process in a FUSE wait.
    let reap: Reap = Arc::new(|child: Child| -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _child = child;
            std::future::pending::<()>().await;
        })
    });
    let view = fixture.view(Some(reap));
    let task = {
        let view = view.clone();
        tokio::spawn(async move { view.snapshot().await })
    };
    fixture.helper_pid().await;
    Wait::new("the deadline").until(|| fixture.clock.pending_sleeps() > 0).await.unwrap();
    let before = fixture.clock.shared().now();
    fixture.clock.advance(DEADLINE + Duration::from_millis(50));
    let taken = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the snapshot returned without the helper's exit")
        .unwrap()
        .unwrap();
    assert_eq!(taken.sizes(1), Some(MountSizes::Timeout));
    let spent = fixture.clock.shared().now().as_millisecond() - before.as_millisecond();
    assert!(spent <= 1050, "{spent} ms of the clock");
    assert!(view.helper_lives(), "the killed helper is not reaped yet");
}

#[tokio::test]
async fn mount_view_no_second_helper_while_first_lives() {
    let fixture = Fixture::new();
    fixture.answer(&["/"]);
    fixture.block();
    let reap: Reap = Arc::new(|child: Child| -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _child = child;
            std::future::pending::<()>().await;
        })
    });
    let view = fixture.view(Some(reap));
    snapshot_past_deadline(&fixture, &view).await;
    assert_eq!(fixture.runs(), 1);
    fixture.unblock();
    fixture.clock.advance(CACHE);
    let taken = view.snapshot().await.unwrap();
    assert_eq!(fixture.runs(), 1, "no second helper while the first lives");
    assert_eq!(taken.sizes(1), Some(MEASURED), "a mount keeps its last sizes");
    assert_eq!(taken.sizes(2), Some(MountSizes::Timeout));
}

#[tokio::test]
async fn mount_view_backoff_skips_timed_out_mount() {
    let fixture = Fixture::new();
    let me = format!("/run/user/{}", fixture.uid);
    let gvfs = format!("{me}/gvfs");
    let others = ["/", "/tmp", me.as_str(), gvfs.as_str()];
    fixture.answer(&others);
    fixture.block();
    let view = fixture.view(None);
    let taken = snapshot_past_deadline(&fixture, &view).await;
    assert_eq!(taken.sizes(10), Some(MountSizes::Timeout));
    Wait::new("the reaper").until(|| !view.helper_lives()).await.unwrap();
    assert_eq!(fixture.runs(), 1);

    fixture.unblock();
    fixture.clock.advance(CACHE);
    let taken = view.snapshot().await.unwrap();
    assert_eq!(fixture.asked(), others, "the slow mount waits out its minute");
    assert_eq!(taken.sizes(10), Some(MountSizes::Timeout));
    assert_eq!(taken.sizes(1), Some(MEASURED));

    assert!(!view.helper_lives(), "a helper that answered every path cannot hang");
    fixture.clock.advance(BACKOFF);
    view.snapshot().await.unwrap();
    assert!(fixture.asked().iter().any(|path| path == "/mnt/slow"), "asked again after it");
}

#[tokio::test]
async fn mount_view_cache_reads_once_in_two_seconds() {
    let fixture = Fixture::new();
    fixture.answer(&["/"]);
    let view = fixture.view(None);
    let first = view.snapshot().await.unwrap();
    fixture.clock.advance(CACHE - Duration::from_millis(1));
    let second = view.snapshot().await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(fixture.runs(), 1);
    fixture.clock.advance(Duration::from_millis(1));
    let third = view.snapshot().await.unwrap();
    assert!(!Arc::ptr_eq(&first, &third));
    assert_eq!(fixture.runs(), 2);
}

#[tokio::test]
async fn a_table_that_does_not_read_fails_the_snapshot() {
    let fixture = Fixture::new();
    let view = MountView::with_parts(
        fixture.dir.join("helper"),
        fixture.clock.shared(),
        fixture.dir.join("no-table"),
        reap_by_waiting(),
    );
    assert!(view.snapshot().await.is_none());
    assert_eq!(fixture.runs(), 0);
}

#[test]
fn mount_view_off_when_mount_namespace_differs() {
    let ns = |link: Option<&str>, ids: Option<&[u32]>| Namespace {
        link: link.map(str::to_owned),
        ids: ids.map(|ids| ids.iter().copied().collect()),
    };
    let a = Some("mnt:[4026531832]");
    let b = Some("mnt:[4026532511]");
    assert!(same_namespace(&ns(a, None), &ns(a, None)));
    assert!(!same_namespace(&ns(a, None), &ns(b, None)));
    // The links decide when both read, also when the ids agree.
    assert!(!same_namespace(&ns(a, Some(&[1, 2])), &ns(b, Some(&[1, 2]))));
    // The parent's link does not read (the systemd user manager): the ids decide.
    assert!(same_namespace(&ns(a, Some(&[1, 2, 3])), &ns(None, Some(&[1, 2, 3]))));
    assert!(!same_namespace(&ns(a, Some(&[1, 2, 3])), &ns(None, Some(&[7, 8, 9]))));
    assert!(!same_namespace(&ns(a, None), &ns(None, None)), "nothing reads: off");
    assert!(!same_namespace(&ns(None, Some(&[])), &ns(None, Some(&[]))));
}

#[test]
fn the_test_process_shares_its_parents_mount_namespace() {
    // The test runner and this test run in one mount namespace.
    assert!(same_mount_namespace());
}

#[test]
fn efrd_unit_keeps_host_mount_namespace() {
    // Each of these gives efrd a mount namespace of its own, and then the table that
    // efrd reads is not the user's (efr's auto spec, 8.8, risk M8).
    const KEYS: &[&str] = &[
        "PrivateTmp",
        "ProtectHome",
        "ProtectSystem",
        "PrivateMounts",
        "ProtectKernelTunables",
        "TemporaryFileSystem",
        "BindPaths",
        "BindReadOnlyPaths",
        "InaccessiblePaths",
        "ReadOnlyPaths",
        "RootDirectory",
    ];
    let unit = include_str!("../../../../../systemd/efrd.service");
    assert!(unit.contains("ExecStart="), "the unit file reads");
    for line in unit.lines().map(str::trim) {
        let key = line.split('=').next().unwrap_or_default().trim();
        assert!(!KEYS.contains(&key), "efrd.service sets {line}");
    }
}

#[test]
fn the_unit_check_reads_a_path_that_exists() {
    let unit = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../systemd/efrd.service");
    assert!(unit.is_file(), "{unit:?}");
}
