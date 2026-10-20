use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    CallId, ConversationId, Event, JobId, JobInfo, JobState, JobStopBy, Launch, NoticeKind, Origin,
    SurfaceChange, TurnId,
};
use efr_store::Batch;
use efr_test_support::{TestClock, TestDirs, TestStore, Wait};
use pretty_assertions::assert_eq;

use super::{JobKey, JobParts, JobTable, NewJob, SpawnEnv, end_of, job_call_dir, sweep};
use crate::sandbox::gc;

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

/// A launcher for these tests: it waits for the start byte, writes `started`, then
/// runs until its lifeline ends, and writes its result.
const LAUNCHER: &str = r#"#!/bin/sh
dir=$3
byte=$(dd bs=1 count=1 2> /dev/null)
if [ -z "$byte" ]; then exit 125; fi
: > "$dir/started"
cat > /dev/null
printf '{"started":true,"exit_code":0,"stop":"lifeline"}' > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
"#;

struct Scene {
    dirs: TestDirs,
    clock: TestClock,
    store: TestStore,
    table: JobTable,
    conversation: ConversationId,
    notices: std::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<super::Notice>>,
    /// The full work that the table counts.
    full: crate::full::FullTurns,
}

impl Scene {
    async fn new() -> Scene {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let store = TestStore::open(clock.shared()).await.unwrap();
        let launcher = dirs.root().join("efr-sbx");
        std::fs::write(&launcher, LAUNCHER).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let full = crate::full::FullTurns::default();
        let (table, notices) = JobTable::new(JobParts {
            clock: clock.shared(),
            writer: store.writer().clone(),
            readers: store.readers().clone(),
            dirs: dirs.dirs().clone(),
            launcher,
            spawn_env: SpawnEnv {
                path: "/usr/local/bin:/usr/bin:/bin".to_owned(),
                home: dirs.home().to_path_buf(),
                user_runtime: dirs.dirs().runtime().to_path_buf(),
                roots: Vec::new(),
            },
            full: full.clone(),
        });
        let conversation = ConversationId::from_uuid(id(1));
        let created = Event::ConversationCreated { origin: Origin::Shell, tty: None };
        store.writer().append(Batch::new().event(conversation, created)).await.unwrap();
        let notices = std::sync::Mutex::new(notices);
        Scene { dirs, clock, store, table, conversation, notices, full }
    }

    fn info(&self, number: u32) -> JobInfo {
        let now = self.clock.now_timestamp();
        JobInfo {
            id: JobId::nth(number),
            conversation_id: self.conversation,
            turn_id: TurnId::from_uuid(id(10)),
            call_id: CallId::from_uuid(id(100 + u128::from(number))),
            command: format!("job {number}"),
            launch: Launch::contained(),
            state: JobState::Running,
            started_at: now,
            ends_by: now.checked_add(Duration::from_secs(3600)).unwrap(),
            ended_at: None,
            exit_code: None,
            stopped_by: None,
            output: self.scratch().join("jobs").join(format!("j{number}.out")),
            output_bytes: 0,
            watch: None,
            wakes: 0,
        }
    }

    fn scratch(&self) -> PathBuf {
        self.dirs.dirs().data().join("scratch/x")
    }

    /// A job with its call dir made, as the toolbox prepares it.
    fn new_job(&self, number: u32) -> NewJob {
        let info = self.info(number);
        let call_dir = job_call_dir(self.dirs.dirs(), self.conversation, info.call_id);
        std::fs::create_dir_all(&call_dir).unwrap();
        std::fs::create_dir_all(self.scratch()).unwrap();
        super::output::make_dir(&self.scratch()).unwrap();
        NewJob {
            info,
            limit: Duration::from_secs(3600),
            state_dir: self.dirs.root().join(format!("state-{number}")),
            call_dir,
            cwd: self.dirs.root().to_path_buf(),
            scratch: self.scratch(),
            matcher: None,
            wakes: 0,
            guard_roots: Vec::new(),
            write_roots: Vec::new(),
            contained: true,
            proxy: None,
            bus: None,
            full: false,
        }
    }

    fn key(&self, number: u32) -> JobKey {
        JobKey { conversation: self.conversation, number }
    }

    /// Starts job `number` through the whole path of the table.
    async fn start(&self, number: u32) -> JobInfo {
        let job = self.new_job(number);
        let mut spawned = self.table.spawn(&job).unwrap();
        self.table.start(&mut spawned, &job).await.unwrap();
        self.table.begin(spawned, job, None).await.unwrap()
    }

    async fn ended(&self, number: u32) -> Event {
        let job = JobId::nth(number);
        Wait::new("the job's end")
            .until_some_async(async || {
                let events = self.store.events().await.unwrap();
                events
                    .into_iter()
                    .map(|envelope| envelope.event)
                    .find(|event| matches!(event, Event::JobEnded { job_id, .. } if *job_id == job))
            })
            .await
            .unwrap()
    }
}

impl Scene {
    /// The next notice that the table handed to the actors.
    async fn notice(&self) -> super::Notice {
        Wait::new("a notice")
            .until_some(|| self.notices.lock().unwrap().try_recv().ok())
            .await
            .unwrap()
    }
}

trait NowTimestamp {
    fn now_timestamp(&self) -> jiff::Timestamp;
}

impl NowTimestamp for TestClock {
    fn now_timestamp(&self) -> jiff::Timestamp {
        efr_stdx::time::Clock::now(self)
    }
}

#[tokio::test]
async fn launcher_waits_for_start_byte_after_peer_list() {
    let scene = Scene::new().await;
    let job = scene.new_job(1);
    let mut spawned = scene.table.spawn(&job).unwrap();
    let pid = spawned.pid();
    // NOTE: no byte was written: the launcher cannot have started anything, and its pid
    // is model-side already.
    assert!(scene.table.model_side().contains(&pid));
    assert!(!job.call_dir.join(efr_sandbox::STARTED_FILE).exists());
    scene.table.start(&mut spawned, &job).await.unwrap();
    assert!(job.call_dir.join(efr_sandbox::STARTED_FILE).exists());
    let info = scene.table.begin(spawned, job, None).await.unwrap();
    assert_eq!(info.state, JobState::Running);
    scene.table.drop_lifeline(scene.key(1));
    scene.ended(1).await;
    assert!(!scene.table.model_side().contains(&pid), "a launcher that ended is not listed");
}

#[tokio::test]
async fn a_job_of_a_full_call_counts_as_full_work_until_it_ends() {
    let scene = Scene::new().await;
    let mut job = scene.new_job(1);
    job.full = true;
    let mut spawned = scene.table.spawn(&job).unwrap();
    scene.table.start(&mut spawned, &job).await.unwrap();
    scene.table.begin(spawned, job, None).await.unwrap();
    scene.start(2).await;
    assert!(scene.full.running(), "the job of a full call counts");

    scene.table.drop_lifeline(scene.key(1));
    scene.ended(1).await;
    Wait::new("the end of the full job").until(|| !scene.full.running()).await.unwrap();

    assert!(scene.table.live(scene.key(2)).is_some(), "a job of another mode does not count");
    scene.table.drop_lifeline(scene.key(2));
    scene.ended(2).await;
}

#[tokio::test]
async fn lifeline_write_end_not_inherited_by_other_jobs() {
    let scene = Scene::new().await;
    scene.start(1).await;
    scene.start(2).await;
    // NOTE: had the second launcher inherited the first lifeline's write end, the first
    // job would run on as long as the second does.
    scene.table.drop_lifeline(scene.key(1));
    let Event::JobEnded { state, stopped_by, .. } = scene.ended(1).await else { unreachable!() };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::Unknown)));
    assert!(scene.table.live(scene.key(2)).is_some(), "the second job still runs");
    scene.table.drop_lifeline(scene.key(2));
    scene.ended(2).await;
}

#[tokio::test]
async fn limit_counts_jobs_that_start() {
    let scene = Scene::new().await;
    let mut settings = efr_config::JobsSettings::default();
    settings.max_running = 1;
    // NOTE: two starts at once: the first holds its place from the check on, before its
    // job runs.
    let first = scene.table.reserve(scene.conversation, &settings).unwrap();
    let refused = scene.table.reserve(scene.conversation, &settings).unwrap_err();
    assert!(refused.contains("1 job runs in this conversation"), "{refused}");
    let other = ConversationId::from_uuid(id(2));
    let mut daemon = settings.clone();
    daemon.max_running_daemon = 1;
    let refused = scene.table.reserve(other, &daemon).unwrap_err();
    assert!(refused.contains("1 job runs in efr"), "{refused}");
    drop(first);
    let again = scene.table.reserve(scene.conversation, &settings);
    assert!(again.is_ok(), "a start that ended frees its place");
}

#[tokio::test]
async fn end_notice_wakes_unless_the_model_stopped_the_job() {
    let scene = Scene::new().await;
    for (number, by, wakes) in [(1, JobStopBy::Model, false), (2, JobStopBy::User, true)] {
        scene.start(number).await;
        scene.table.stop(scene.key(number), by).await.unwrap();
        let notice = scene.notice().await;
        assert_eq!(notice.input.kind, NoticeKind::Exited);
        assert_eq!(notice.input.wakes, wakes, "the end notice of a job that {by:?} stopped");
        assert_eq!(notice.input.job.stopped_by, Some(by));
    }
    scene.start(3).await;
    scene.table.drop_lifeline(scene.key(3));
    let notice = scene.notice().await;
    assert!(notice.input.wakes, "a job that ended on its own wakes the conversation");
}

#[tokio::test]
async fn sweep_keeps_session_until_no_process_has_it() {
    let scene = Scene::new().await;
    let dir = tempfile::tempdir().unwrap();
    let Ok(mut child) = efr_stdx::process::command("setsid", dir.path())
        .arg("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    else {
        return;
    };
    let session = child.id().unwrap();
    Wait::new("the new session").until(|| sweep::has_members(session)).await.unwrap();
    scene.table.keep_session(session);
    scene.table.scan_once().await;
    assert!(scene.table.model_side().contains(&session), "a process still has the session");
    tokio::task::spawn_blocking(move || sweep::kill_session(session)).await.unwrap();
    let _ = child.wait().await;
    scene.table.scan_once().await;
    assert!(!scene.table.model_side().contains(&session), "no process has it any more");
}

/// Records the start of job `number` as the last efrd did, with `pid` and `start` as its
/// launcher.
async fn started_before(scene: &Scene, number: u32, pid: u32, start: u64) -> JobInfo {
    let info = scene.info(number);
    record_start(scene, info, pid, start).await
}

/// [`started_before`] for a job that started at `started_at` on the real clock.
async fn started_at(
    scene: &Scene,
    number: u32,
    pid: u32,
    start: u64,
    started_at: jiff::Timestamp,
) -> JobInfo {
    let info = JobInfo { started_at, ..scene.info(number) };
    record_start(scene, info, pid, start).await
}

async fn record_start(scene: &Scene, info: JobInfo, pid: u32, start: u64) -> JobInfo {
    let event = Event::JobStarted {
        turn_id: info.turn_id,
        call_id: info.call_id,
        job: info.clone(),
        launcher_pid: pid,
        launcher_start: start,
    };
    scene.store.writer().append(Batch::new().event(scene.conversation, event)).await.unwrap();
    info
}

#[tokio::test]
async fn restart_kills_leftover_launcher_only_with_same_start_time() {
    let scene = Scene::new().await;
    let spawn = || {
        efr_stdx::process::command("sleep", scene.dirs.root())
            .arg("30")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    let (mut same, mut other) = (spawn(), spawn());
    let (same_pid, other_pid) = (same.id().unwrap(), other.id().unwrap());
    let start = sweep::start_time(same_pid).unwrap();
    started_before(&scene, 1, same_pid, start).await;
    // NOTE: a pid whose start time differs belongs to another process now; a file that
    // a job wrote could claim anything, but the time comes from the event.
    started_before(&scene, 2, other_pid, sweep::start_time(other_pid).unwrap() + 1).await;
    scene.table.recover().await.unwrap();
    let status = same.wait().await.unwrap();
    assert!(!status.success(), "the launcher with its own start time was killed");
    assert!(sweep::process(Path::new("/proc"), other_pid).is_some(), "the other pid lives");
    other.kill().await.unwrap();
}

/// A session whose leader ended: `setsid sh` starts `sleep` and ends, so the sleep keeps
/// the session of the shell's pid, as the orphan of a lost launcher does. Returns the
/// session, the start time of its leader and the sleep's pid; `None` without `setsid`.
async fn lost_leader(dir: &Path) -> Option<(u32, u64, u32)> {
    use tokio::io::AsyncBufReadExt as _;
    let mut leader = efr_stdx::process::command("setsid", dir)
        .args(["sh", "-c", "sleep 30 < /dev/null > /dev/null 2>&1 & echo $!; read -r _"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let session = leader.id().unwrap();
    let start = sweep::start_time(session).unwrap();
    let stdout = leader.stdout.take().unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(stdout).read_line(&mut line).await.unwrap();
    let orphan: u32 = line.trim().parse().unwrap();
    drop(leader.stdin.take());
    leader.wait().await.unwrap();
    let found = sweep::process(Path::new("/proc"), orphan).unwrap();
    assert_eq!(found.session, session, "the orphan keeps the session of its leader");
    Some((session, start, orphan))
}

/// True once `pid` is gone, or a zombie that its new parent did not reap yet.
fn gone(pid: u32) -> bool {
    sweep::process(Path::new("/proc"), pid).is_none_or(|process| process.state == 'Z')
}

#[tokio::test]
async fn restart_spares_session_of_reused_pid() {
    let scene = Scene::new().await;
    // NOTE: a session leader that is not the job's launcher, such as a terminal's
    // shell or a tmux server that got the launcher's pid after a boot.
    let Ok(mut leader) = efr_stdx::process::command("setsid", scene.dirs.root())
        .args(["sleep", "30"])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    else {
        return;
    };
    let pid = leader.id().unwrap();
    Wait::new("the new session").until(|| sweep::has_members(pid)).await.unwrap();
    let start = sweep::start_time(pid).unwrap();
    started_at(&scene, 1, pid, start + 1, jiff::Timestamp::now()).await;
    scene.table.recover().await.unwrap();
    assert!(!gone(pid), "a session that another process leads now is not swept");
    assert!(!scene.table.model_side().contains(&pid));
    leader.kill().await.unwrap();
}

#[tokio::test]
async fn restart_sweeps_session_of_lost_launcher_in_same_boot() {
    let scene = Scene::new().await;
    let Some((session, start, orphan)) = lost_leader(scene.dirs.root()).await else { return };
    started_at(&scene, 1, session, start, jiff::Timestamp::now()).await;
    scene.table.recover().await.unwrap();
    Wait::new("the swept orphan").until(|| gone(orphan)).await.unwrap();
}

#[tokio::test]
async fn restart_spares_session_from_another_boot() {
    let scene = Scene::new().await;
    let Some((session, start, orphan)) = lost_leader(scene.dirs.root()).await else { return };
    // NOTE: a job that started a day before the launcher seems to have started: the pid
    // and its session belong to a later boot.
    let day_before = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
    started_at(&scene, 1, session, start, day_before).await;
    scene.table.recover().await.unwrap();
    assert!(!gone(orphan), "a session of another boot is not swept");
    sweep::kill(orphan, rustix::process::Signal::KILL);
    Wait::new("the orphan's end").until(|| gone(orphan)).await.unwrap();
}

#[tokio::test]
async fn restart_spares_processes_older_than_the_launcher() {
    let scene = Scene::new().await;
    let Some((session, _, orphan)) = lost_leader(scene.dirs.root()).await else { return };
    // NOTE: a launcher that started after every process of the session started none
    // of them.
    let later = sweep::start_time(orphan).unwrap() + 1;
    started_at(&scene, 1, session, later, jiff::Timestamp::now()).await;
    scene.table.recover().await.unwrap();
    assert!(!gone(orphan), "a process older than the launcher is not the job's");
    sweep::kill(orphan, rustix::process::Signal::KILL);
    Wait::new("the orphan's end").until(|| gone(orphan)).await.unwrap();
}

#[tokio::test]
async fn restart_survivor_session_is_model_side() {
    let scene = Scene::new().await;
    let Some((session, start, orphan)) = lost_leader(scene.dirs.root()).await else { return };
    started_at(&scene, 1, session, start, jiff::Timestamp::now()).await;
    // NOTE: no process outlives SIGKILL on demand, so the sweep here leaves the orphan
    // and names it as a survivor, as a sweep that a process outlived does.
    scene.table.recover_by(|_, _, _| vec!["sleep".to_owned()]).await.unwrap();
    let Event::JobEnded { survivors, .. } = scene.ended(1).await else { unreachable!() };
    assert_eq!(survivors, ["sleep"]);
    assert!(scene.table.model_side().contains(&session), "the survivor's session is model-side");
    scene.table.scan_once().await;
    assert!(scene.table.model_side().contains(&session), "a process still has the session");
    sweep::kill(orphan, rustix::process::Signal::KILL);
    Wait::new("the orphan's end").until(|| gone(orphan)).await.unwrap();
    scene.table.scan_once().await;
    assert!(!scene.table.model_side().contains(&session), "no process has it any more");
}

#[tokio::test]
async fn job_reported_stopped_after_efrd_restart() {
    let scene = Scene::new().await;
    let info = started_before(&scene, 1, 0, 0).await;
    std::fs::create_dir_all(info.output.parent().unwrap()).unwrap();
    std::fs::write(&info.output, "one\ntwo\n").unwrap();
    scene.table.recover().await.unwrap();
    let Event::JobEnded { state, stopped_by, launcher_lost, tail, output_truncated, .. } =
        scene.ended(1).await
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::DaemonRestart)));
    assert!(launcher_lost, "no result.json was left");
    assert!(output_truncated);
    assert_eq!(tail, "one\ntwo");
    let rows = scene.store.readers().with(efr_store::jobs::running).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn restart_reads_leftover_result() {
    let scene = Scene::new().await;
    let info = started_before(&scene, 1, 0, 0).await;
    let call_dir = job_call_dir(scene.dirs.dirs(), scene.conversation, info.call_id);
    std::fs::create_dir_all(&call_dir).unwrap();
    let change = SurfaceChange {
        path: PathBuf::from("/home/u/p/.git/hooks/pre-commit"),
        rule: "new_hook".to_owned(),
        key: None,
        quarantined: true,
    };
    let result = efr_sandbox::SandboxResult {
        started: true,
        exit_code: Some(0),
        summary: efr_protocol::SandboxSummary {
            survivors: vec!["sudo".to_owned()],
            surface_changes: vec![change.clone()],
            ..efr_protocol::SandboxSummary::default()
        },
        ..efr_sandbox::SandboxResult::default()
    };
    std::fs::write(call_dir.join(efr_sandbox::RESULT_FILE), result.to_json().unwrap()).unwrap();
    scene.table.recover().await.unwrap();
    let Event::JobEnded { survivors, surface_changes, quarantined, launcher_lost, .. } =
        scene.ended(1).await
    else {
        unreachable!()
    };
    assert_eq!(survivors, ["sudo"]);
    assert_eq!(surface_changes, [change]);
    assert!(quarantined);
    assert!(!launcher_lost);
    assert!(!call_dir.exists(), "the job dirs of the last efrd go");
}

#[tokio::test]
async fn undelivered_notices_sent_at_start() {
    let scene = Scene::new().await;
    started_before(&scene, 1, 0, 0).await;
    scene.table.recover().await.unwrap();
    let notices = scene.table.undelivered().await.unwrap();
    let [notice] = notices.try_into().unwrap();
    assert_eq!(notice.conversation, scene.conversation);
    assert_eq!(notice.input.kind, NoticeKind::Exited);
    assert!(!notice.input.wakes, "a notice of a restart never starts a turn");
    assert!(
        notice.input.text.starts_with("<job id=\"j1\" state=\"stopped\""),
        "{}",
        notice.input.text
    );
}

#[tokio::test]
async fn job_numbers_follow_the_store_and_never_repeat() {
    let scene = Scene::new().await;
    started_before(&scene, 4, 0, 0).await;
    assert_eq!(scene.table.number(scene.conversation).await.unwrap(), 5);
    assert_eq!(scene.table.number(scene.conversation).await.unwrap(), 6);
    let other = ConversationId::from_uuid(id(2));
    assert_eq!(scene.table.number(other).await.unwrap(), 1);
}

#[tokio::test]
async fn bus_proxy_of_a_job_lives_with_the_job() {
    let scene = Scene::new().await;
    let fake = crate::sandbox::bus::fake::FakeBus::new(&scene.dirs.root().join("fake"));
    let bus = fake.service(scene.clock.shared());
    let mut job = scene.new_job(1);
    let started =
        bus.start(&job.call_dir, &[efr_protocol::BusKind::System], &fake.upstream()).await.unwrap();
    let pid = started.pids()[0];
    job.bus = Some(started);
    let mut spawned = scene.table.spawn(&job).unwrap();
    scene.table.start(&mut spawned, &job).await.unwrap();
    scene.table.begin(spawned, job, None).await.unwrap();

    let alive = || Path::new(&format!("/proc/{pid}")).exists();
    assert!(alive(), "the filter runs while the job runs");
    assert_eq!(fake.ended(), 0);
    scene.table.drop_lifeline(scene.key(1));
    scene.ended(1).await;

    assert_eq!(fake.ended(), 1, "the filter ended with the job, by its closed pipe");
    Wait::new("the filter's exit").until(|| !alive()).await.unwrap();
}

#[test]
fn job_call_dir_not_seen_as_running_call() {
    let dirs = TestDirs::new().unwrap();
    let conversation = ConversationId::from_uuid(id(1));
    let job_dir = job_call_dir(dirs.dirs(), conversation, CallId::from_uuid(id(5)));
    std::fs::create_dir_all(&job_dir).unwrap();
    std::fs::write(job_dir.join(efr_sandbox::STARTED_FILE), b"").unwrap();
    let shell_dir = job_dir.parent().unwrap().parent().unwrap();
    assert!(!gc::launcher_running(shell_dir), "a running job is no running call");
    let call_dir = shell_dir.join(CallId::from_uuid(id(6)).to_string());
    std::fs::create_dir_all(&call_dir).unwrap();
    std::fs::write(call_dir.join(efr_sandbox::STARTED_FILE), b"").unwrap();
    assert!(gc::launcher_running(shell_dir));
}

#[test]
fn cache_collector_keeps_running_job_layers() {
    let dirs = TestDirs::new().unwrap();
    let root = dirs.dirs().state().join("sandbox");
    let layers = root.join("conv/jobs/call/cache/cargo/upper");
    std::fs::create_dir_all(&layers).unwrap();
    std::fs::write(layers.join("big"), vec![0_u8; 4096]).unwrap();
    // NOTE: the collector reads only `<conversation>/cache`; a job's layers in
    // `<conversation>/jobs/<call>` are its own and go with the job.
    let now = std::time::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let found = gc::scan(&root, now, &|_| false);
    assert!(found.is_empty(), "{found:?}");
    assert!(layers.join("big").exists());
}

#[test]
fn end_of_a_job_from_its_result_and_its_stop() {
    let result = |exit_code, stop| efr_sandbox::SandboxResult {
        started: true,
        exit_code,
        stop,
        ..efr_sandbox::SandboxResult::default()
    };
    let ended = end_of(Some(&result(Some(3), None)), None);
    assert_eq!((ended.state, ended.exit_code), (JobState::Exited, Some(3)));
    let limit = end_of(Some(&result(None, Some(efr_sandbox::JobStop::Deadline))), None);
    assert_eq!((limit.state, limit.stopped_by), (JobState::TimedOut, Some(JobStopBy::TimeLimit)));
    let asked = end_of(Some(&result(Some(143), None)), Some(JobStopBy::Model));
    assert_eq!(
        (asked.state, asked.exit_code, asked.stopped_by),
        (JobState::Stopped, None, Some(JobStopBy::Model))
    );
    let timed = end_of(None, Some(JobStopBy::TimeLimit));
    assert_eq!(timed.state, JobState::TimedOut);
    assert_eq!(end_of(None, None).state, JobState::Failed);
    let setup = efr_sandbox::SandboxResult {
        setup_error: Some("no".to_owned()),
        ..efr_sandbox::SandboxResult::default()
    };
    assert_eq!(end_of(Some(&setup), None).state, JobState::Failed);
    let _ = Arc::new(());
}

#[tokio::test]
async fn restart_writes_a_notice_for_each_ended_job() {
    let scene = Scene::new().await;
    let other = ConversationId::from_uuid(id(2));
    let created =
        Event::ConversationCreated { origin: Origin::Shell, tty: Some("/dev/pts/5".to_owned()) };
    scene.store.writer().append(Batch::new().event(other, created)).await.unwrap();
    for number in [1, 2] {
        let info = JobInfo { conversation_id: other, ..scene.info(number) };
        let event = Event::JobStarted {
            turn_id: info.turn_id,
            call_id: info.call_id,
            job: info.clone(),
            launcher_pid: 0,
            launcher_start: 0,
        };
        scene.store.writer().append(Batch::new().event(other, event)).await.unwrap();
    }
    // A job of a conversation with no terminal gets no line.
    started_before(&scene, 3, 0, 0).await;

    let ended = scene.table.recover().await.unwrap();
    let dir = scene.dirs.dirs().runtime().join(crate::notices::NOTICES_DIR);
    super::restart_notices(scene.store.readers(), ended, &dir).await;

    let text = std::fs::read_to_string(dir.join("pts-5")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let line = |job: &str| {
        format!(
            "efr: job {job} stopped because efrd restarted: a conversation; its launcher was \
             lost, and its processes may still run"
        )
    };
    assert_eq!(lines, [line("j1"), line("j2")]);
    let files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name()).collect();
    assert_eq!(files, ["pts-5"], "the conversation without a terminal gets none");
}

impl Scene {
    /// Starts job `number` as a call of full mode starts it.
    async fn start_full(&self, number: u32) -> JobInfo {
        let mut job = self.new_job(number);
        job.full = true;
        job.contained = false;
        job.info.launch = Launch::Unsandboxed;
        let mut spawned = self.table.spawn(&job).unwrap();
        self.table.start(&mut spawned, &job).await.unwrap();
        self.table.begin(spawned, job, None).await.unwrap()
    }
}

#[tokio::test]
async fn gate_off_stops_full_jobs_only() {
    let scene = Scene::new().await;
    scene.start_full(1).await;
    scene.start(2).await;
    scene.start_full(3).await;

    let stopped = scene.table.stop_full();

    assert_eq!(stopped, [scene.key(1), scene.key(3)]);
    for number in [1, 3] {
        let Event::JobEnded { state, stopped_by, .. } = scene.ended(number).await else {
            unreachable!()
        };
        assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::FullOff)));
        let notice = scene.notice().await;
        assert_eq!(notice.input.job.stopped_by, Some(JobStopBy::FullOff));
        assert!(!notice.input.wakes, "the end of a full job at the gate wakes no turn");
    }
    assert!(scene.table.live(scene.key(2)).is_some(), "a job of another mode runs on");
    Wait::new("the end of the full work").until(|| !scene.full.running()).await.unwrap();
    scene.table.drop_lifeline(scene.key(2));
    scene.ended(2).await;
}

#[tokio::test]
async fn full_job_that_starts_after_the_gate_went_off_stops_at_once() {
    let scene = Scene::new().await;
    // NOTE: the reload marks full mode off before it looks at the table; a full job
    // that enters the table after that look must stop on its own.
    scene.full.set_off(true);
    assert_eq!(scene.table.stop_full(), []);

    scene.start_full(1).await;
    scene.start(2).await;

    let Event::JobEnded { stopped_by, .. } = scene.ended(1).await else { unreachable!() };
    assert_eq!(stopped_by, Some(JobStopBy::FullOff));
    assert!(scene.table.live(scene.key(2)).is_some(), "a job of another mode starts");
    scene.table.drop_lifeline(scene.key(2));
    scene.ended(2).await;
}
