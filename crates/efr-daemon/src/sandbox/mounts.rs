//! The mount view (efr's auto spec, section 8.8): efrd's snapshot of the host's mounts
//! for a call that runs `df` or `findmnt`.
//!
//! efrd runs in the user's mount namespace ([`same_mount_namespace`]), so its own
//! `/proc/self/mountinfo` is the user's table. It measures each mount outside the
//! sandbox in one helper process, `efr-sbx statvfs`, with one deadline for the whole
//! snapshot: a FUSE or NFS mount that hangs holds a thread in a wait that only SIGKILL
//! ends, and a thread of efrd in such a wait could never be stopped. At the deadline
//! efrd kills the helper and hands it to a reaper task, so a snapshot never waits for
//! the helper's exit. At most one helper lives at a time; while one lives, a mount
//! keeps its last sizes, or none. A mount that missed the deadline is not measured
//! again for a minute, and a snapshot serves every call for two seconds.
//!
//! The rules of what gets sizes are `efr_sandbox`'s: no `statvfs` for the dummy types
//! (a `statvfs` of `autofs` can mount a network share), none for another user's
//! mounts, and per call none for the mounts at or below its masks.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_sandbox::{
    MAX_MOUNTS_BYTES, MountEntry, MountSizes, MountSnapshot, StatvfsLine, is_dummy, is_masked,
    is_other_users, parse_mountinfo,
};
use efr_stdx::time::Clock;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::Child;

/// The table that efrd reads: its own, which is the user's.
const OWN_TABLE: &str = "/proc/self/mountinfo";

/// How long the whole measure may take.
const DEADLINE: Duration = Duration::from_secs(1);

/// How long a snapshot serves later calls.
const CACHE: Duration = Duration::from_secs(2);

/// How long a mount that missed the deadline is not measured again.
const BACKOFF: Duration = Duration::from_secs(60);

/// `EILSEQ`: a target that is not UTF-8, which the helper's answers cannot name.
const EILSEQ: i32 = 84;

/// What a reaper does with a helper once efrd stopped reading it: wait for its exit.
pub(crate) type Reap = Arc<dyn Fn(Child) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A mount as the backoff and the last sizes know it: its device and its target.
type Key = (u32, u32, PathBuf);

/// What the helper gave for one mount: its sizes, and the device that `stat` gave.
type Answer = (MountSizes, Option<(u32, u32)>);

/// efrd's snapshots of the host's mounts. Cheap to clone; clones share the cache.
#[derive(Clone)]
pub(crate) struct MountView {
    inner: Arc<Inner>,
}

struct Inner {
    helper: PathBuf,
    table: PathBuf,
    clock: Arc<dyn Clock>,
    reap: Reap,
    uid: u32,
    state: Mutex<State>,
    /// One snapshot at a time, so calls that come together read the table once.
    taking: tokio::sync::Semaphore,
}

#[derive(Default)]
struct State {
    cached: Option<(i64, Arc<Taken>)>,
    /// Until when (milliseconds since the epoch) a mount is not measured.
    backoff: HashMap<Key, i64>,
    /// The last sizes that each mount gave.
    last: HashMap<Key, Answer>,
    /// The number of the helper that may still hang: one that efrd killed at the
    /// deadline and that has not ended yet, or one that runs now.
    live: Option<u64>,
    /// The helpers started so far, which number them.
    helpers: u64,
}

impl std::fmt::Debug for MountView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountView").field("helper", &self.inner.helper).finish_non_exhaustive()
    }
}

/// One snapshot: the table as read, and the sizes of its mounts.
#[derive(Debug)]
pub(crate) struct Taken {
    table: Vec<u8>,
    entries: Vec<MountEntry>,
    sizes: BTreeMap<u32, MountSizes>,
    /// The device that `stat` gave, where it differs from the table's.
    devices: BTreeMap<u32, (u32, u32)>,
    taken_at: u64,
}

impl Taken {
    /// The two files of a call: `mounts` (the table, verbatim) and `mounts.json` (the
    /// sizes), with no sizes for a mount at or below one of `masks` or strictly below
    /// one of `below`.
    pub(crate) fn files(&self, masks: &[PathBuf], below: &[PathBuf]) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut sizes = self.sizes.clone();
        for entry in &self.entries {
            if is_masked(&entry.target, masks, below) && !is_dummy(&entry.fstype) {
                sizes.insert(entry.id, MountSizes::Masked);
            }
        }
        let snapshot = MountSnapshot::new(self.taken_at, sizes).with_devices(self.devices.clone());
        let json = snapshot.to_json().ok()?;
        Some((self.table.clone(), json))
    }

    /// The sizes of the mount `id`.
    #[cfg(test)]
    pub(crate) fn sizes(&self, id: u32) -> Option<MountSizes> {
        self.sizes.get(&id).copied()
    }
}

impl MountView {
    /// The view that measures with `helper` (the launcher's copy, `$R/bin/efr-sbx`, or
    /// a test's program) and keeps time with `clock`.
    pub(crate) fn new(helper: PathBuf, clock: Arc<dyn Clock>) -> MountView {
        MountView::with_parts(helper, clock, PathBuf::from(OWN_TABLE), reap_by_waiting())
    }

    /// The view that reads `table` in place of efrd's own and gives each helper that
    /// it stopped reading to `reap`.
    pub(crate) fn with_parts(
        helper: PathBuf,
        clock: Arc<dyn Clock>,
        table: PathBuf,
        reap: Reap,
    ) -> MountView {
        MountView {
            inner: Arc::new(Inner {
                helper,
                table,
                clock,
                reap,
                uid: rustix::process::getuid().as_raw(),
                state: Mutex::new(State::default()),
                taking: tokio::sync::Semaphore::new(1),
            }),
        }
    }

    /// True while a helper process lives.
    #[cfg(test)]
    pub(crate) fn helper_lives(&self) -> bool {
        self.state().live.is_some()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // NOTE: every value is replaced whole, so a poisoned lock still holds usable ones.
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The snapshot of now: the one of the last two seconds, or a new one. `None` when
    /// the table could not be read. It returns within the deadline, and never waits
    /// for a helper to end.
    pub(crate) async fn snapshot(&self) -> Option<Arc<Taken>> {
        // NOTE: the permit lives across the read and the measure, so a second call that
        // comes meanwhile waits for this snapshot and takes it from the cache. The
        // semaphore is never closed; without a permit the snapshot still runs.
        let _one = self.inner.taking.acquire().await.ok();
        let now = self.inner.clock.now().as_millisecond();
        if let Some((at, taken)) = &self.state().cached
            && now.saturating_sub(*at) < millis(CACHE)
        {
            return Some(Arc::clone(taken));
        }
        let path = self.inner.table.clone();
        let table = match tokio::task::spawn_blocking(move || read_table(&path)).await {
            Ok(Ok(table)) => table,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "the mount table could not be read");
                return None;
            }
            Err(_) => return None,
        };
        let entries = parse_mountinfo(&table);
        let mut sizes = BTreeMap::new();
        let mut devices = BTreeMap::new();
        let mut wanted: Vec<(u32, Key, String)> = Vec::new();
        {
            let state = self.state();
            for entry in &entries {
                let key = (entry.major, entry.minor, entry.target.clone());
                let sized = if is_dummy(&entry.fstype) {
                    Some(MountSizes::Dummy)
                } else if is_other_users(entry, self.inner.uid) {
                    Some(MountSizes::OtherUser)
                } else if entry.target.to_str().is_none() {
                    Some(MountSizes::Failed { errno: EILSEQ })
                } else if state.backoff.get(&key).is_some_and(|until| *until > now) {
                    Some(MountSizes::Timeout)
                } else {
                    None
                };
                match sized {
                    Some(sized) => {
                        sizes.insert(entry.id, sized);
                    }
                    None => {
                        let target = entry.target.to_string_lossy().into_owned();
                        wanted.push((entry.id, key, target));
                    }
                }
            }
        }
        if !wanted.is_empty() {
            let answers = self.measure(&wanted, now).await;
            for ((id, key, _), (sized, device)) in wanted.iter().zip(answers) {
                sizes.insert(*id, sized);
                if let Some(device) = device.filter(|device| *device != (key.0, key.1)) {
                    devices.insert(*id, device);
                }
            }
        }
        let taken = Arc::new(Taken {
            table,
            entries,
            sizes,
            devices,
            taken_at: u64::try_from(now / 1000).unwrap_or_default(),
        });
        self.state().cached = Some((now, Arc::clone(&taken)));
        Some(taken)
    }

    /// Measures `wanted` with one helper: an answer for each, in order. While an older
    /// helper lives, it starts none: each mount keeps its last sizes, or gets `timeout`.
    async fn measure(&self, wanted: &[(u32, Key, String)], now: i64) -> Vec<Answer> {
        let started = {
            let mut state = self.state();
            if state.live.is_some() {
                None
            } else {
                state.helpers += 1;
                state.live = Some(state.helpers);
                Some(state.helpers)
            }
        };
        let (answers, timed_out) = match started {
            Some(number) => self.run_helper(wanted, number).await,
            None => {
                tracing::debug!("a statvfs helper still runs; the mounts keep their last sizes");
                let state = self.state();
                return wanted
                    .iter()
                    .map(|(_, key, _)| {
                        state.last.get(key).copied().unwrap_or((MountSizes::Timeout, None))
                    })
                    .collect();
            }
        };
        let mut state = self.state();
        wanted
            .iter()
            .map(|(_, key, target)| match answers.get(target) {
                Some(answer) => {
                    state.backoff.remove(key);
                    state.last.insert(key.clone(), *answer);
                    *answer
                }
                None if timed_out => {
                    state.backoff.insert(key.clone(), now.saturating_add(millis(BACKOFF)));
                    (MountSizes::Timeout, None)
                }
                None => (MountSizes::Failed { errno: 0 }, None),
            })
            .collect()
    }

    /// Runs the helper `number` for the targets of `wanted` until it ends or the
    /// deadline passes. Returns the answers by target, and whether the deadline passed.
    async fn run_helper(
        &self,
        wanted: &[(u32, Key, String)],
        number: u64,
    ) -> (HashMap<String, Answer>, bool) {
        let mut answers = HashMap::new();
        let mut command = efr_stdx::process::command(&self.inner.helper, "/");
        command
            .arg("statvfs")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(error = %error, helper = %self.inner.helper.display(), "the statvfs helper could not start");
                self.inner.ended(number);
                return (answers, false);
            }
        };
        let mut input: Vec<u8> = Vec::new();
        let mut targets: BTreeSet<&str> = BTreeSet::new();
        for (_, _, target) in wanted {
            if targets.insert(target) {
                input.extend_from_slice(target.as_bytes());
                input.push(0);
            }
        }
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let read = async {
            let write = async {
                if let Some(mut stdin) = stdin {
                    let _ = stdin.write_all(&input).await;
                }
            };
            let lines = async {
                let Some(stdout) = stdout else { return };
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(answer) = serde_json::from_str::<StatvfsLine>(&line) {
                        answers.insert(answer.path.clone(), (answer.sizes(), answer.device));
                    }
                    if answers.len() >= targets.len() {
                        break;
                    }
                }
            };
            tokio::join!(write, lines);
        };
        let timed_out = self.inner.clock.timeout(DEADLINE, read).await.is_err();
        if timed_out {
            tracing::warn!("a mount did not answer statvfs within a second; efrd kills the helper");
            let _ = child.start_kill();
        } else {
            // NOTE: it answered every path or closed its output, so no thread of it
            // waits for a mount, and the next snapshot may start a helper.
            self.inner.ended(number);
        }
        // NOTE: never wait here: a process in a FUSE or NFS wait can take seconds to end
        // after SIGKILL. The reaper waits, and after a deadline the next helper starts
        // only after it.
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            (inner.reap)(child).await;
            inner.ended(number);
        });
        (answers, timed_out)
    }
}

impl Inner {
    /// The helper `number` can no longer hang: a later one may start.
    fn ended(&self, number: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.live == Some(number) {
            state.live = None;
        }
    }
}

/// The reaper of efrd: it waits for the helper's exit in a task of its own.
pub(crate) fn reap_by_waiting() -> Reap {
    Arc::new(|mut child: Child| -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _ = child.wait().await;
        })
    })
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// The table at `path`, at most [`MAX_MOUNTS_BYTES`]. It blocks.
fn read_table(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_MOUNTS_BYTES).unwrap_or(u64::MAX)).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// What efrd knows about one process's mount namespace: the target of its
/// `ns/mnt` link, and the ids of its mounts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Namespace {
    /// `mnt:[<inode>]`, when the link reads. The kernel lets a process read the link
    /// of another only when it may trace it, and the systemd user manager holds a
    /// capability that efrd lacks, so under systemd the parent's link does not read.
    pub(crate) link: Option<String>,
    /// The mount ids of its `mountinfo`, which every process may read. Mount ids are
    /// unique on the machine, so two namespaces share none.
    pub(crate) ids: Option<BTreeSet<u32>>,
}

impl Namespace {
    /// The namespace of the process whose `/proc` dir is `proc`. It blocks.
    fn read(proc: &Path) -> Namespace {
        let link = std::fs::read_link(proc.join("ns/mnt"))
            .ok()
            .map(|target| target.to_string_lossy().into_owned());
        let ids = std::fs::read(proc.join("mountinfo"))
            .ok()
            .map(|table| parse_mountinfo(&table).iter().map(|entry| entry.id).collect());
        Namespace { link, ids }
    }
}

/// True when `own` and `parent` are the same mount namespace: by their links when
/// both read, else by their mount ids.
pub(crate) fn same_namespace(own: &Namespace, parent: &Namespace) -> bool {
    if let (Some(own), Some(parent)) = (&own.link, &parent.link) {
        return own == parent;
    }
    match (&own.ids, &parent.ids) {
        (Some(own), Some(parent)) => !own.is_empty() && own == parent,
        _ => false,
    }
}

/// True when efrd's mount namespace is the one of its parent process: the systemd
/// user manager, or the shell that started efrd. A unit with `PrivateTmp=`,
/// `ProtectHome=` or another mount key gives efrd a namespace of its own, and then
/// efrd's table is not the user's: the mount view stays off, and `df` stays an exit
/// that runs outside. It blocks: efrd calls it once at start in `spawn_blocking`.
pub(crate) fn same_mount_namespace() -> bool {
    let Some(parent) = rustix::process::getppid() else {
        tracing::warn!("efrd has no parent process; the mount view is off");
        return false;
    };
    let parent = Path::new("/proc").join(parent.as_raw_pid().to_string());
    let own = Path::new("/proc/self");
    // NOTE: a mount that comes or goes between the reads changes the ids; read again.
    for _ in 0..3 {
        let before = Namespace::read(own);
        let theirs = Namespace::read(&parent);
        if Namespace::read(own) != before {
            continue;
        }
        let same = same_namespace(&before, &theirs);
        if !same {
            tracing::warn!(
                own = ?before.link,
                parent = ?theirs.link,
                "efrd runs in another mount namespace than its parent; the mount view is off"
            );
        }
        return same;
    }
    tracing::warn!("the mount table changed while efrd compared it; the mount view is off");
    false
}

#[cfg(test)]
mod tests;
