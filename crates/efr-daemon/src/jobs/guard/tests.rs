use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::Duration;

use efr_protocol::{
    CallId, ConversationId, Event, JobId, JobInfo, JobState, JobStopBy, Launch, Origin,
    SurfaceChange, TurnId,
};
use efr_stdx::time::Clock as _;
use efr_store::Batch;
use efr_test_support::{TestClock, TestDirs, TestStore, Wait};
use pretty_assertions::assert_eq;

use super::{GUARD_INTERVAL, overlap};
use crate::jobs::{JobParts, JobTable, NewJob, SpawnEnv, job_call_dir, job_state_dir};

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

/// A launcher for these tests. In job mode it waits for the start byte, writes
/// `started` and runs until its lifeline ends. `guard-check` counts its runs in
/// `guard-runs` of the conversation's sandbox dir, and finds a planted hook while the
/// state dir holds `plant`.
const LAUNCHER: &str = r#"#!/bin/sh
if [ "$1" = guard-check ]; then
  state=$5
  echo run >> "$state/../../guard-runs"
  if [ -e "$state/plant" ]; then
    printf '%s' '{"changes":[{"path":"/p/sub/.git/hooks/pre-commit","rule":"new_hook","quarantined":true}],"quarantined":true}' > "$state/guard.json"
    exit 3
  fi
  printf '%s' '{"changes":[],"quarantined":false}' > "$state/guard.json"
  exit 0
fi
dir=$3
byte=$(dd bs=1 count=1 2> /dev/null)
if [ -z "$byte" ]; then exit 125; fi
: > "$dir/started"
cat > /dev/null
printf '{"started":true,"exit_code":0,"stop":"signal"}' > "$dir/result.tmp"
mv "$dir/result.tmp" "$dir/result.json"
"#;

/// The change that the test launcher's guard check finds.
fn planted() -> SurfaceChange {
    SurfaceChange {
        path: PathBuf::from("/p/sub/.git/hooks/pre-commit"),
        rule: "new_hook".to_owned(),
        key: None,
        quarantined: true,
    }
}

struct Scene {
    dirs: TestDirs,
    clock: TestClock,
    store: TestStore,
    table: JobTable,
    conversation: ConversationId,
}

impl Scene {
    async fn new() -> Scene {
        let dirs = TestDirs::new().unwrap();
        let clock = TestClock::new();
        let store = TestStore::open(clock.shared()).await.unwrap();
        let launcher = dirs.root().join("efr-sbx");
        std::fs::write(&launcher, LAUNCHER).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (table, _notices) = JobTable::new(JobParts {
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
            full: crate::full::FullTurns::default(),
            host_ports: std::sync::Arc::new(crate::sandbox::seams::TestHostPorts::new()),
        });
        let conversation = ConversationId::from_uuid(id(1));
        let created = Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None };
        store.writer().append(Batch::new().event(conversation, created)).await.unwrap();
        Scene { dirs, clock, store, table, conversation }
    }

    fn info(&self, number: u32) -> JobInfo {
        let now = self.clock.now();
        let scratch = self.dirs.dirs().data().join("scratch/x");
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
            output: scratch.join("jobs").join(format!("j{number}.out")),
            output_bytes: 0,
            watch: None,
            wakes: 0,
            forwards: Vec::new(),
        }
    }

    /// The state dir of job `number`, as efrd makes it.
    fn state_dir(&self, number: u32) -> PathBuf {
        job_state_dir(self.dirs.dirs(), self.conversation, self.info(number).call_id)
    }

    /// Starts job `number`, contained, with `roots` as its guard roots.
    async fn start(&self, number: u32, roots: &[&str]) -> JobInfo {
        let info = self.info(number);
        let call_dir = job_call_dir(self.dirs.dirs(), self.conversation, info.call_id);
        std::fs::create_dir_all(&call_dir).unwrap();
        let state_dir = self.state_dir(number);
        std::fs::create_dir_all(&state_dir).unwrap();
        let scratch = info.output.parent().unwrap().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&scratch).unwrap();
        crate::jobs::output::make_dir(&scratch).unwrap();
        let job = NewJob {
            info,
            limit: Duration::from_secs(3600),
            call_dir,
            state_dir,
            cwd: self.dirs.root().to_path_buf(),
            scratch,
            matcher: None,
            wakes: 0,
            guard_roots: roots.iter().map(PathBuf::from).collect(),
            write_roots: roots.iter().map(PathBuf::from).collect(),
            contained: true,
            proxy: None,
            bus: None,
            forward: None,
            full: false,
        };
        let mut spawned = self.table.spawn(&job).unwrap();
        self.table.start(&mut spawned, &job).await.unwrap();
        self.table.begin(spawned, job, None).await.unwrap()
    }

    /// How many guard checks ran for the jobs of the conversation.
    fn guard_runs(&self) -> usize {
        let runs = self.state_dir(1).parent().unwrap().parent().unwrap().join("guard-runs");
        std::fs::read_to_string(runs).map(|text| text.lines().count()).unwrap_or(0)
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

#[test]
fn roots_overlap_when_one_holds_the_other() {
    let roots = |paths: &[&str]| paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    assert!(overlap(&roots(&["/p/app"]), &roots(&["/p/app"])));
    assert!(overlap(&roots(&["/p"]), &roots(&["/q", "/p/app/src"])));
    assert!(overlap(&roots(&["/p/app/src"]), &roots(&["/p/app"])));
    assert!(!overlap(&roots(&["/p/app"]), &roots(&["/p/apps", "/q"])));
    assert!(!overlap(&roots(&[]), &roots(&["/p"])));
}

#[tokio::test]
async fn guard_break_stops_job() {
    let scene = Scene::new().await;
    scene.start(1, &["/p"]).await;
    // A clean check changes nothing; a check of other roots does not run.
    scene.table.check_guards(None).await;
    assert_eq!(scene.guard_runs(), 1);
    assert!(
        scene
            .table
            .live(crate::jobs::JobKey { conversation: scene.conversation, number: 1 })
            .is_some()
    );
    std::fs::write(scene.state_dir(1).join("plant"), "").unwrap();
    scene.table.check_guards(Some(&[PathBuf::from("/q")])).await;
    assert_eq!(scene.guard_runs(), 1, "the job's roots do not overlap /q");

    scene.table.check_guards(Some(&[PathBuf::from("/p/sub")])).await;
    let Event::JobEnded { state, stopped_by, surface_changes, quarantined, .. } =
        scene.ended(1).await
    else {
        unreachable!()
    };
    assert_eq!((state, stopped_by), (JobState::Stopped, Some(JobStopBy::Guard)));
    assert_eq!(surface_changes, [planted()]);
    assert!(quarantined);
}

#[tokio::test]
async fn guard_runs_every_five_minutes() {
    let scene = Scene::new().await;
    scene.start(1, &["/p"]).await;
    // NOTE: the job's own time limit sleeps too.
    scene.clock.wait_for_sleeps(1).await;
    tokio::spawn(scene.table.clone().guard_schedule());
    // NOTE: a check sleeps too, for its own time limit; the schedule is asleep again
    // once it asked for one more interval.
    let intervals =
        || scene.clock.requested_sleeps().iter().filter(|sleep| **sleep == GUARD_INTERVAL).count();
    Wait::new("the schedule's first sleep").until(|| intervals() == 1).await.unwrap();
    assert_eq!(scene.guard_runs(), 0);
    for runs in 1..=2 {
        scene.clock.advance(GUARD_INTERVAL);
        Wait::new("a guard check").until(|| scene.guard_runs() == runs).await.unwrap();
        Wait::new("the schedule's next sleep").until(|| intervals() == runs + 1).await.unwrap();
    }
    scene.table.drop_lifeline(crate::jobs::JobKey { conversation: scene.conversation, number: 1 });
    scene.ended(1).await;
}

/// Records the start of job `number` as the last efrd did, with no launcher left.
async fn started_before(scene: &Scene, number: u32) -> JobInfo {
    let info = scene.info(number);
    let event = Event::JobStarted {
        turn_id: info.turn_id,
        call_id: info.call_id,
        job: info.clone(),
        launcher_pid: 0,
        launcher_start: 0,
    };
    scene.store.writer().append(Batch::new().event(scene.conversation, event)).await.unwrap();
    info
}

#[tokio::test]
async fn restart_runs_guard_of_lost_job() {
    let scene = Scene::new().await;
    started_before(&scene, 1).await;
    let state_dir = scene.state_dir(1);
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("plant"), "").unwrap();
    scene.table.recover().await.unwrap();
    assert_eq!(scene.guard_runs(), 1, "the lost job's guard ran before its dirs went");
    let Event::JobEnded { stopped_by, surface_changes, quarantined, launcher_lost, .. } =
        scene.ended(1).await
    else {
        unreachable!()
    };
    assert_eq!(stopped_by, Some(JobStopBy::DaemonRestart));
    assert_eq!(surface_changes, [planted()]);
    assert!(quarantined && launcher_lost);
    assert!(!state_dir.exists(), "the job's state dir went after the check");
}

/// An exit child job left no guard record: nothing runs for it.
#[tokio::test]
async fn restart_runs_no_guard_for_a_lost_exit_child_job() {
    let scene = Scene::new().await;
    let mut info = scene.info(1);
    info.launch = Launch::Unsandboxed;
    let event = Event::JobStarted {
        turn_id: info.turn_id,
        call_id: info.call_id,
        job: info.clone(),
        launcher_pid: 0,
        launcher_start: 0,
    };
    scene.store.writer().append(Batch::new().event(scene.conversation, event)).await.unwrap();
    std::fs::create_dir_all(scene.state_dir(1)).unwrap();
    scene.table.recover().await.unwrap();
    assert_eq!(scene.guard_runs(), 0);
}
