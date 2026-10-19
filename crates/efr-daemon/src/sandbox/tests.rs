//! The sandbox service: the launcher's copy, the probe's status and its event, and the
//! call dir of a prepared call. The launcher is a script that answers the probe from a
//! file, so no kernel sandbox runs here.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{Approver, Engine, ExitNeed, Locations, WriteBind};
use efr_protocol::{
    CallId, ConversationId, DesktopEnv, DesktopKind, Event, ExitKind, ExitSource, Grant, Launch,
    ProjectId, Scope, TurnId,
};
use efr_sandbox::{
    DesktopSocket, JobPlan, Mask, MaskKind, MountSnapshot, ProbeFailure, ProbeReport, SandboxSpec,
    SpecLaunch, parse_mountinfo, parse_nonce_hex,
};
use efr_scope::{Git, Home};
use efr_stdx::paths::Dirs;
use efr_test_support::{TestClock, TestRng, TestStore};

use super::{
    HostFacts, JobInput, MOUNT_VIEW_FAILED, NOT_PROBED, PrepareInput, SandboxService, Seams,
    ServiceParts,
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    dirs: Dirs,
    store: TestStore,
    /// The file whose text the fake launcher prints as its report.
    report: PathBuf,
    /// One line per probe the fake launcher answered.
    probes: PathBuf,
}

impl Fixture {
    async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let dirs =
            Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r")).unwrap();
        for made in [&home.join("p/app"), &dirs.config().to_path_buf(), &root.join("lib")] {
            std::fs::create_dir_all(made).unwrap();
        }
        let report = root.join("report.json");
        let probes = root.join("probes");
        let script = format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\ncat '{}'\n",
            probes.display(),
            report.display()
        );
        let launcher = root.join("lib/efr-sbx");
        std::fs::write(&launcher, script).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = TestStore::open(TestClock::new().shared()).await.unwrap();
        let fixture = Fixture { _dir: dir, root, home, dirs, store, report, probes };
        fixture.answer(&ProbeReport {
            bwrap: Some("/usr/bin/bwrap".into()),
            landlock_abi: Some(10),
            ..ProbeReport::default()
        });
        fixture
    }

    fn answer(&self, report: &ProbeReport) {
        std::fs::write(&self.report, report.to_json().unwrap()).unwrap();
    }

    fn probes(&self) -> usize {
        std::fs::read_to_string(&self.probes).unwrap_or_default().lines().count()
    }

    async fn service(&self) -> SandboxService {
        let clock = TestClock::new().shared();
        SandboxService::new(ServiceParts {
            dirs: self.dirs.clone(),
            home: Home::new(&self.home).unwrap(),
            host: HostFacts {
                path: "/usr/bin:/bin".to_owned(),
                user_runtime: self.root.join("xrt"),
                ..HostFacts::default()
            },
            source: Some(self.root.join("lib/efr-sbx")),
            seams: Seams::default(),
            registry: self.dirs.config().join("projects.toml"),
            writer: self.store.writer().clone(),
            clock: Arc::clone(&clock),
            rng: Arc::new(TestRng::new(3)),
            git: Git::new(clock).isolated(),
        })
        .await
    }
}

fn unavailable_events(events: &[efr_protocol::EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::SandboxUnavailable { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn probe_failure_marks_unavailable() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let status = service.status();
    assert_eq!(status.borrow().reason.as_deref(), Some(NOT_PROBED));
    // The copy lies in efr's runtime root, read-only, and is what runs.
    let copy = fixture.dirs.runtime().join("bin/efr-sbx");
    assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o500);

    let settings = Settings::default();
    service.probe(&settings).await;
    assert!(status.borrow().available, "{:?}", *status.borrow());
    assert_eq!(fixture.probes(), 1);

    let failure = ProbeFailure::UsernsOff;
    fixture.answer(&ProbeReport { failure: Some(failure.clone()), ..ProbeReport::default() });
    service.probe(&settings).await;
    let now = status.borrow().clone();
    assert!(!now.available);
    assert_eq!(now.reason, Some(failure.reason()));
    // The same failure again records nothing new.
    service.probe(&settings).await;
    let events = fixture.store.events().await.unwrap();
    assert_eq!(unavailable_events(&events), [failure.reason()]);
    // Turned off, the launcher is not asked.
    let mut off = Settings::default();
    off.sandbox.enabled = false;
    service.probe(&off).await;
    assert_eq!(fixture.probes(), 3);
    assert_eq!(status.borrow().reason.as_deref(), Some("sandbox.enabled = false"));
}

#[tokio::test]
async fn a_changed_launcher_is_refused_until_a_restart_copies_it() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    std::fs::write(fixture.root.join("lib/efr-sbx"), "#!/bin/sh\necho changed\n").unwrap();
    service.probe(&Settings::default()).await;
    assert_eq!(service.current().reason, Some(ProbeFailure::LauncherMismatch.reason()),);
    let paths = service.paths().await;
    assert_eq!(paths.launcher_sha256_ok, Some(false));
    assert_eq!(paths.launcher, Some(fixture.dirs.runtime().join("bin/efr-sbx")));
    assert_eq!(paths.runtime, fixture.dirs.runtime().join("sbx"));
    assert_eq!(paths.state, fixture.dirs.state().join("sandbox"));
}

fn call(fixture: &Fixture, launch: Launch, exits: Vec<ExitNeed>) -> CallContext {
    call_n(fixture, 3, launch, exits)
}

/// The call `n` of the conversation.
fn call_n(fixture: &Fixture, n: u128, launch: Launch, exits: Vec<ExitNeed>) -> CallContext {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    CallContext::new(
        ConversationId::from_uuid(id(1)),
        TurnId::from_uuid(id(2)),
        CallId::from_uuid(id(n)),
        fixture.home.join("p/app"),
        fixture.dirs.data().join("scratch/x"),
    )
    .with_scope(Scope::Project(ProjectId::from_uuid(id(9))))
    .with_launch(launch)
    .with_exits(exits)
}

#[tokio::test]
async fn a_prepared_call_has_a_private_call_dir_with_its_spec_and_nonce() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let settings = Settings::default();
    let locations = Locations::new(&fixture.home)
        .unwrap()
        .with_project(ProjectId::from_uuid(uuid::Uuid::from_u128(9)), fixture.home.join("p/app"))
        .unwrap();
    let engine = Engine::with_defaults(locations);
    let input = PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view: false,
    };

    let before = service.prepare(&call(&fixture, Launch::contained(), Vec::new()), &input).await;
    assert!(matches!(before, Err(crate::DaemonError::SandboxUnavailable { .. })), "{before:?}");

    service.probe(&settings).await;
    let target = fixture.home.join("notes.txt");
    let need = ExitNeed {
        kind: ExitKind::Write,
        grants: vec![Grant::Write { path: target.clone() }],
        part: "echo hi > ~/notes.txt".to_owned(),
        approver: Approver::Classifier,
        names: Vec::new(),
        hosts: Vec::new(),
        backup_before_write: false,
        package_fetch: None,
        source: ExitSource::Predicted,
        target: Some(target.clone()),
        bind: Some(WriteBind::MakeFile),
        link: None,
        copies: Vec::new(),
        covered: None,
        installs: Vec::new(),
    };
    let launch = Launch::Contained { grants: vec![Grant::Write { path: target.clone() }] };
    let prepared = service.prepare(&call(&fixture, launch, vec![need]), &input).await.unwrap();
    let dir = &prepared.run.dir;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir), 0o700);
    assert_eq!(mode(&dir.join("spec.json")), 0o600);
    assert_eq!(mode(&dir.join("nonce")), 0o600);
    let nonce = std::fs::read_to_string(dir.join("nonce")).unwrap();
    assert_eq!(parse_nonce_hex(&nonce), Some(prepared.run.nonce));
    let spec = SandboxSpec::from_json(&std::fs::read(dir.join("spec.json")).unwrap()).unwrap();
    assert_eq!(spec.launch, SpecLaunch::Contained);
    assert_eq!(spec.grants, [Grant::Write { path: target.clone() }]);
    assert_eq!(spec.runtime.call_dir, *dir);
    assert_eq!(spec.runtime.launcher, fixture.dirs.runtime().join("bin/efr-sbx"));
    assert_eq!(spec.write_roots[0].path, fixture.home.join("p/app"));
    assert!(target.is_file(), "efrd made the target that the grant binds");
    assert_eq!(prepared.projects, [fixture.home.join("p/app")]);
    assert_eq!(prepared.started, dir.join("started"));
    // The same call id cannot be prepared twice: its files exist.
    drop(prepared);
    let again = service.prepare(&call(&fixture, Launch::Unsandboxed, Vec::new()), &input).await;
    assert!(again.is_err());
}

#[tokio::test]
async fn a_desktop_grant_binds_the_socket_of_the_prompts_terminal() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let settings = Settings::default();
    service.probe(&settings).await;
    let engine = Engine::with_defaults(Locations::new(&fixture.home).unwrap());
    let input = PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view: false,
    };
    let instance = fixture.root.join("xrt/hypr/abc_1760000000_1");
    std::fs::create_dir_all(&instance).unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(instance.join(".socket.sock")).unwrap();
    let launch =
        Launch::Contained { grants: vec![Grant::Desktop { desktop: DesktopKind::Hyprland }] };
    let conversation = ConversationId::from_uuid(uuid::Uuid::from_u128(1));

    // efrd's own start environment names no Hyprland, and no prompt sent one.
    let refused = service.prepare(&call(&fixture, launch.clone(), Vec::new()), &input).await;
    let Err(error) = refused else { panic!("refused: {refused:?}") };
    assert_eq!(
        efr_stdx::with_causes(&error),
        "the sandbox could not plan the call: efr found no Hyprland session in the terminal of \
         this prompt"
    );

    let desktop = DesktopEnv {
        hyprland_instance_signature: Some("abc_1760000000_1".to_owned()),
        ..DesktopEnv::default()
    };
    service.set_desktop(conversation, Some(&desktop));
    let prepared = service.prepare(&call(&fixture, launch, Vec::new()), &input).await.unwrap();
    let spec = SandboxSpec::from_json(&std::fs::read(prepared.run.dir.join("spec.json")).unwrap())
        .unwrap();
    assert_eq!(
        spec.runtime.desktops,
        [DesktopSocket {
            desktop: DesktopKind::Hyprland,
            path: instance.join(".socket.sock"),
            value: "abc_1760000000_1".to_owned(),
        }]
    );
}

/// The engine of the fixture's project.
fn project_engine(fixture: &Fixture) -> Engine {
    let locations = Locations::new(&fixture.home)
        .unwrap()
        .with_project(ProjectId::from_uuid(uuid::Uuid::from_u128(9)), fixture.home.join("p/app"))
        .unwrap();
    Engine::with_defaults(locations)
}

/// The times that the fake launcher ran as the `statvfs` helper.
fn statvfs_runs(fixture: &Fixture) -> usize {
    let runs = std::fs::read_to_string(&fixture.probes).unwrap_or_default();
    runs.lines().filter(|line| *line == "statvfs").count()
}

/// Checks the two files of the mount view in `dir`: private, the table of efrd's
/// mount namespace, and the sizes.
fn assert_mount_files(dir: &Path) {
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir.join("mounts")), 0o600);
    assert_eq!(mode(&dir.join("mounts.json")), 0o600);
    let table = parse_mountinfo(&std::fs::read(dir.join("mounts")).unwrap());
    assert!(table.iter().any(|entry| entry.target == Path::new("/")), "{table:?}");
    MountSnapshot::from_json(&std::fs::read(dir.join("mounts.json")).unwrap()).unwrap();
}

#[tokio::test]
async fn mount_view_only_for_mount_lines() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    assert!(service.mount_view(), "the test runs in the mount namespace of its parent");
    let settings = Settings::default();
    service.probe(&settings).await;
    let engine = project_engine(&fixture);
    let input = |mount_view| PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view,
    };

    let (no_df, with_df) = (input(false), input(true));
    let first = call_n(&fixture, 3, Launch::contained(), Vec::new());
    let plain = service.prepare(&first, &no_df).await.unwrap();
    assert!(!plain.run.dir.join("mounts").exists());
    assert!(!plain.run.dir.join("mounts.json").exists());
    assert_eq!(statvfs_runs(&fixture), 0, "no snapshot for a line without df");
    // The plan lock of the project goes with the prepared call.
    drop(plain);

    let second = call_n(&fixture, 4, Launch::contained(), Vec::new());
    let df = service.prepare(&second, &with_df).await.unwrap();
    assert_mount_files(&df.run.dir);
    assert_eq!(statvfs_runs(&fixture), 1);
    assert!(!df.notes.contains(&MOUNT_VIEW_FAILED.to_owned()));
    drop(df);

    // The exit child runs on the host, where df is already right.
    let outside = call_n(&fixture, 5, Launch::Unsandboxed, Vec::new());
    let outside = service.prepare(&outside, &with_df).await.unwrap();
    assert!(!outside.run.dir.join("mounts").exists());
}

#[tokio::test]
async fn mount_view_job_gets_snapshot_at_start() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let settings = Settings::default();
    service.probe(&settings).await;
    let engine = project_engine(&fixture);
    let input = PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view: true,
    };
    let job = JobInput {
        plan: JobPlan::new(60, 19),
        line: "while :; do df -h; sleep 60; done",
        env: b"",
    };
    let call = call_n(&fixture, 6, Launch::contained(), Vec::new());
    let prepared = service.prepare_job(&call, &input, job).await.unwrap();
    assert!(prepared.run.dir.starts_with(service.shell_dir(call.conversation_id).join("jobs")));
    assert_mount_files(&prepared.run.dir);
}

#[tokio::test]
async fn mount_view_failure_adds_a_note() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let settings = Settings::default();
    service.probe(&settings).await;
    let engine = project_engine(&fixture);
    let input = PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view: false,
    };
    let first = call(&fixture, Launch::contained(), Vec::new());
    let prepared = service.prepare(&first, &input).await.unwrap();
    let spec = SandboxSpec::from_json(&std::fs::read(prepared.run.dir.join("spec.json")).unwrap());
    let spec = spec.unwrap();

    let failed = tokio::spawn(async { None });
    let mut notes = Vec::new();
    let files = super::mount_files(failed, &spec, &mut notes).await;
    assert!(files.is_empty(), "no file: the call's df shows the sandbox's mounts");
    assert_eq!(notes, [MOUNT_VIEW_FAILED]);
}

#[tokio::test]
async fn an_unmask_grant_gives_the_mounts_below_its_path_their_sizes() {
    let fixture = Fixture::new().await;
    let service = fixture.service().await;
    let settings = Settings::default();
    service.probe(&settings).await;
    let engine = project_engine(&fixture);
    let input = PrepareInput {
        settings: &settings,
        engine: &engine,
        named_paths: Vec::new(),
        mount_view: false,
    };
    let first = call(&fixture, Launch::contained(), Vec::new());
    let prepared = service.prepare(&first, &input).await.unwrap();
    let mut spec =
        SandboxSpec::from_json(&std::fs::read(prepared.run.dir.join("spec.json")).unwrap())
            .unwrap();
    let (vault, notes) = (fixture.home.join(".vault"), fixture.home.join("notes"));
    spec.masks.push(Mask { path: vault.clone(), kind: MaskKind::User });
    spec.masks.push(Mask { path: notes.clone(), kind: MaskKind::User });
    spec.grants.push(Grant::Unmask { path: notes.clone() });

    let (masks, below) = super::mount_masks(&spec);
    assert!(masks.contains(&vault));
    assert!(!masks.contains(&notes), "the unmasked path shows its sizes");
    assert_eq!(
        below,
        [
            spec.runtime.user_runtime.clone(),
            spec.runtime.runtime.clone(),
            spec.runtime.data.clone(),
            spec.runtime.state.clone()
        ]
    );
}
