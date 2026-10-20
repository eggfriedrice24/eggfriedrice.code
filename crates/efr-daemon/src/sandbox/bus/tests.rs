//! The bus filter's supervisor below the turn: the spawn through the launcher's
//! `dbus-proxy`, the ready byte, the lifetime of a call's proxies, the upstream check,
//! and the prepare path with a spec that holds `Grant::BusFilter`. The program is the
//! fake of `fake.rs`; no D-Bus tool runs. One test runs the real launcher and the real
//! `xdg-dbus-proxy` with a stand-in bus, under `just test-sandbox`.

use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{BusRule, Engine, Locations};
use efr_protocol::{
    BusKind, BusProxyOff, CallId, ConversationId, Grant, Launch, ProjectId, Scope, TurnId,
};
use efr_sandbox::{JobPlan, ProbeReport, SandboxSpec};
use efr_scope::{Git, Home};
use efr_stdx::paths::Dirs;
use efr_stdx::time::{Clock, SystemClock};
use efr_test_support::{TestClock, TestRng, TestStore, Wait};
use pretty_assertions::assert_eq;

use super::fake::FakeBus;
use super::{BusUpstream, NO_SESSION_BUS, NO_SYSTEM_BUS};
use crate::error::BusProxyProblem;
use crate::sandbox::{HostFacts, JobInput, PrepareInput, SandboxService, Seams, ServiceParts};
use crate::{DaemonError, Deps};

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

/// The value after `--call=` of each argument, in order.
fn calls(start: &[String]) -> Vec<&str> {
    start.iter().filter_map(|arg| arg.strip_prefix("--call=")).collect()
}

/// True while the process `pid` runs (a zombie has ended).
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| !rest.starts_with('Z')))
        .unwrap_or(false)
}

#[tokio::test]
async fn bus_proxy_args_reach_the_program() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let service = fake.service(system_clock());
    let call_dir = root.join("call");
    std::fs::create_dir(&call_dir).unwrap();

    let call = service.start(&call_dir, &[BusKind::System], &fake.upstream()).await.unwrap();

    let starts = fake.starts();
    assert_eq!(starts.len(), 1);
    let start = &starts[0];
    assert_eq!(start[0], call_dir.join("bus").to_string_lossy(), "its working dir");
    let address = format!("unix:path={}", fake.system.display());
    assert_eq!(start[1..4], ["--fd=3", address.as_str(), "system.sock"]);
    assert_eq!(start[4..6], ["--filter", "--log"]);
    let rules = calls(start);
    assert!(rules.contains(&"org.freedesktop.hostname1=org.freedesktop.DBus.Properties.GetAll@/org/freedesktop/hostname1"), "{rules:?}");
    assert!(start.iter().all(|arg| !arg.starts_with("--talk") && !arg.starts_with("--own")));
    let dir_mode = std::fs::metadata(call_dir.join("bus")).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    let report = call.finish().await;
    assert!(!report.stopped);
    assert_eq!(fake.ended(), 1, "the proxy ended by itself when its sync pipe closed");
}

#[tokio::test]
async fn bus_proxy_waits_for_the_ready_byte() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let clock = TestClock::new();
    let service = fake.service(clock.shared());
    let call_dir = root.join("call");
    std::fs::create_dir(&call_dir).unwrap();
    fake.set("hold-ready");

    let starting = {
        let (service, call_dir, upstream) = (service.clone(), call_dir.clone(), fake.upstream());
        tokio::spawn(async move { service.start(&call_dir, &[BusKind::Session], &upstream).await })
    };
    Wait::new("the program's start").until(|| fake.starts().len() == 1).await.unwrap();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!starting.is_finished(), "no ready byte yet");
    fake.unset("hold-ready");
    let call = starting.await.unwrap().unwrap();
    call.finish().await;

    // With no byte the wait ends at its limit on the injected clock.
    fake.set("hold-ready");
    let second = root.join("second");
    std::fs::create_dir(&second).unwrap();
    let starting = {
        let (service, upstream) = (service.clone(), fake.upstream());
        tokio::spawn(async move { service.start(&second, &[BusKind::Session], &upstream).await })
    };
    Wait::new("the second start").until(|| fake.starts().len() == 2).await.unwrap();
    Wait::new("the ready wait")
        .until(|| {
            let waits = clock.requested_sleeps();
            waits.iter().filter(|wait| **wait == super::READY_WAIT).count() == 2
        })
        .await
        .unwrap();
    clock.advance(super::READY_WAIT);
    let failed = starting.await.unwrap().unwrap_err();
    assert!(
        matches!(failed, DaemonError::BusProxy { problem: BusProxyProblem::NoReadyByte }),
        "{failed:?}"
    );
    fake.unset("hold-ready");
}

#[tokio::test]
async fn bus_proxy_relative_socket_in_a_deep_call_dir() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let service = fake.service(system_clock());
    let call_dir = root.join("a".repeat(60)).join("b".repeat(60)).join("call");
    std::fs::create_dir_all(&call_dir).unwrap();
    assert!(call_dir.join("bus/session.sock").as_os_str().len() > 107);

    let call = service.start(&call_dir, &[BusKind::Session], &fake.upstream()).await.unwrap();

    let start = &fake.starts()[0];
    assert_eq!(start[0], call_dir.join("bus").to_string_lossy());
    assert_eq!(start[3], "session.sock", "a name relative to its working dir");
    call.finish().await;
}

#[tokio::test]
async fn bus_proxy_removes_a_stale_socket() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let service = fake.service(system_clock());
    let call_dir = root.join("call");
    std::fs::create_dir_all(call_dir.join("bus")).unwrap();
    drop(UnixListener::bind(call_dir.join("bus/system.sock")).unwrap());
    assert!(call_dir.join("bus/system.sock").exists(), "a socket file that a proxy left");

    let call = service.start(&call_dir, &[BusKind::System], &fake.upstream()).await.unwrap();

    let start = &fake.starts()[0];
    assert_eq!(start.last().map(String::as_str), Some("files:"), "{start:?}");
    call.finish().await;
}

#[tokio::test]
async fn bus_proxy_start_failure_fails_the_call() {
    let fixture = Fixture::new(Buses::Both).await;
    fixture.fake.set("no-ready");
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };

    let failed = fixture.prepare(launch).await.unwrap_err();

    assert!(
        matches!(failed, DaemonError::BusProxy { problem: BusProxyProblem::NoReadyByte }),
        "{failed:?}"
    );
    assert!(!fixture.call_dir().exists(), "the call dir of a call that did not start goes");
    assert_eq!(fixture.fake.starts().len(), 1);

    // No program: the call fails too, and never runs with the real bus.
    let service = fixture.service_with(Arc::default()).await;
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };
    let failed = fixture.prepare_on(&service, launch).await.unwrap_err();
    assert!(matches!(failed, DaemonError::BusProxy { problem: BusProxyProblem::NoProgram }));
}

#[tokio::test]
async fn bus_proxy_ends_with_the_call() {
    let fixture = Fixture::new(Buses::Both).await;
    let launch = Launch::Contained {
        grants: vec![
            Grant::BusFilter { bus: BusKind::System },
            Grant::BusFilter { bus: BusKind::Session },
        ],
    };

    let mut prepared = fixture.prepare(launch).await.unwrap();

    let spec = fixture.spec();
    assert!(spec.grants.contains(&Grant::BusFilter { bus: BusKind::System }));
    assert!(spec.grants.contains(&Grant::BusFilter { bus: BusKind::Session }));
    let bus = prepared.bus.take().unwrap();
    // NOTE: the plan lock goes with the rest, as after the launcher's start.
    drop(prepared);
    let pids = bus.pids();
    assert_eq!(pids.len(), 2);
    assert!(pids.iter().all(|pid| alive(*pid)));
    let report = bus.finish().await;
    assert!(!report.stopped);
    assert_eq!(fixture.fake.ended(), 2);
    assert!(pids.iter().all(|pid| !alive(*pid)));

    // A call that drops its proxies kills them.
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };
    let prepared = fixture.prepare_n(4, launch).await.unwrap();
    let pids = prepared.bus.as_ref().unwrap().pids();
    drop(prepared);
    Wait::new("the killed proxy").until(|| !alive(pids[0])).await.unwrap();
}

#[tokio::test]
async fn bus_proxy_of_a_timed_out_call_ends_at_sweep() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let service = fake.service(system_clock());
    let call_dir = root.join("call");
    std::fs::create_dir(&call_dir).unwrap();
    let call = service.start(&call_dir, &[BusKind::System], &fake.upstream()).await.unwrap();
    let pid = call.pids()[0];

    service.keep_timed_out(call, call_dir.clone());

    assert_eq!(service.sweep().await, 0, "the launcher wrote no result yet");
    assert!(alive(pid));
    std::fs::write(call_dir.join(efr_sandbox::RESULT_FILE), b"{}").unwrap();
    assert_eq!(service.sweep().await, 1);
    assert_eq!(service.held(), 0);
    assert_eq!(fake.ended(), 1);
    assert!(!alive(pid));
}

#[tokio::test]
async fn bus_proxy_reports_a_crash_as_stopped() {
    let (_temp, root) = temp();
    let fake = FakeBus::new(&root.join("fake"));
    let service = fake.service(system_clock());
    let call_dir = root.join("call");
    std::fs::create_dir(&call_dir).unwrap();
    fake.set("crash");

    let call = service.start(&call_dir, &[BusKind::System], &fake.upstream()).await.unwrap();
    let pid = call.pids()[0];
    Wait::new("the crash").until(|| !alive(pid)).await.unwrap();
    let report = call.finish().await;

    assert!(report.stopped);
    assert_eq!(fake.ended(), 0);
}

#[tokio::test]
async fn bus_proxy_refuses_an_upstream_in_efr_dirs() {
    let (_temp, root) = temp();
    let runtime = root.join("r");
    let project = root.join("p");
    let outside = root.join("o");
    for dir in [&runtime, &project, &outside] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let in_efr = runtime.join("bus");
    let in_project = project.join("bus");
    let good = outside.join("bus");
    let _listeners: Vec<UnixListener> = [&in_efr, &in_project, &good]
        .iter()
        .map(|path| UnixListener::bind(path).unwrap())
        .collect();
    let file = outside.join("file");
    std::fs::write(&file, b"").unwrap();
    let link = outside.join("link");
    std::os::unix::fs::symlink(&in_efr, &link).unwrap();
    let refused = [runtime.clone(), project.clone()];

    let check = |path: &Path| BusUpstream::check(Some(path), None, &refused).system;

    assert_eq!(check(&in_efr), None, "efr's runtime root");
    assert_eq!(check(&in_project), None, "a write root");
    assert_eq!(check(&file), None, "not a socket");
    assert_eq!(check(&link), None, "a link into efr's dirs counts by its target");
    assert_eq!(check(&outside.join("missing")), None);
    assert_eq!(check(&good), Some(good.clone()));
    let both = BusUpstream::check(None, Some(&good), &refused);
    assert_eq!(both.path(BusKind::Session), Some(good.as_path()));
    assert_eq!(both.path(BusKind::System), None);
}

#[tokio::test]
async fn bus_rules_reload_reaches_the_next_call() {
    let fixture = Fixture::new(Buses::Both).await;
    let bus = fixture.service.bus();
    let follower = Follower::start(&fixture);
    let first_dir = fixture.dirs.runtime().join("first");
    std::fs::create_dir_all(&first_dir).unwrap();
    let first = bus.start(&first_dir, &[BusKind::System], &fixture.fake.upstream()).await.unwrap();

    let mut settings = fixture.settings.clone();
    settings.dbus.allow = vec![BusRule {
        bus: BusKind::System,
        name: "org.freedesktop.timedate1".to_owned(),
        call: vec!["org.freedesktop.timedate1.ListTimezones".to_owned()],
    }];
    follower.settings.send_replace(Arc::new(settings));
    Wait::new("the new rules").until(|| !bus.rules().is_empty()).await.unwrap();
    let second_dir = fixture.dirs.runtime().join("second");
    std::fs::create_dir_all(&second_dir).unwrap();
    let second =
        bus.start(&second_dir, &[BusKind::System], &fixture.fake.upstream()).await.unwrap();

    let starts = fixture.fake.starts();
    let added = "org.freedesktop.timedate1=org.freedesktop.timedate1.ListTimezones";
    assert!(!calls(&starts[0]).contains(&added), "a running call keeps the rules of its start");
    assert!(calls(&starts[1]).contains(&added), "the next call gets the new rule");
    first.finish().await;
    second.finish().await;
    follower.stop().await;
}

#[tokio::test]
async fn bus_proxy_self_test_turns_it_on() {
    let mut fixture = Fixture::new(Buses::Both).await;
    fixture.settings.dbus.proxy = true;

    fixture.service.probe(&fixture.settings).await;

    let status = fixture.service.current();
    assert_eq!((status.bus_proxy, status.bus_proxy_off), (true, None));
    assert_eq!(fixture.fake.versions(), 1);
    let start = &fixture.fake.starts()[0];
    let test_dirs = fixture.dirs.runtime().join("sbx/bus-test");
    assert!(start[0].starts_with(&*test_dirs.to_string_lossy()), "{start:?}");
    assert!(start[2].ends_with("/no-bus"), "an upstream that no client uses: {start:?}");
    assert_eq!(fixture.fake.ended(), 1, "it ended by itself when its pipe closed");
    assert_eq!(std::fs::read_dir(&test_dirs).unwrap().count(), 0, "its dir goes");
    let reported = fixture.service.reported(&fixture.settings);
    assert!(reported.bus_proxy);
    assert!(reported.warnings.is_empty(), "{:?}", reported.warnings);
}

#[tokio::test]
async fn no_self_test_while_dbus_proxy_is_off() {
    let fixture = Fixture::new(Buses::Both).await;
    assert!(!fixture.settings.dbus.proxy);

    fixture.service.probe(&fixture.settings).await;

    let status = fixture.service.current();
    assert_eq!((status.bus_proxy, status.bus_proxy_off), (false, Some(BusProxyOff::Config)));
    assert_eq!(fixture.fake.versions(), 0);
    assert!(fixture.fake.starts().is_empty());
}

#[tokio::test]
async fn bus_proxy_off_without_the_program() {
    let mut fixture = Fixture::new(Buses::Both).await;
    fixture.settings.dbus.proxy = true;
    fixture.settings.set_source("dbus.proxy", efr_config::Source::File);
    let service = fixture.service_with(Arc::default()).await;

    let status = service.current();
    assert_eq!((status.bus_proxy, status.bus_proxy_off), (false, Some(BusProxyOff::NoProgram)));
    let reported = service.reported(&fixture.settings);
    assert_eq!(reported.bus_proxy_off, Some(BusProxyOff::NoProgram));
    assert!(
        reported.warnings.iter().any(|warning| warning.contains("xdg-dbus-proxy is not installed")),
        "{:?}",
        reported.warnings
    );
    // With `dbus.proxy` true only by its default, the optional program is no warning;
    // the status still says why the filter is off.
    fixture.settings.set_source("dbus.proxy", efr_config::Source::Default);
    let reported = service.reported(&fixture.settings);
    assert_eq!((reported.bus_proxy, reported.bus_proxy_off), (false, Some(BusProxyOff::NoProgram)));
    assert!(reported.warnings.is_empty(), "{:?}", reported.warnings);
}

#[tokio::test]
async fn bus_proxy_off_when_self_test_fails() {
    let mut fixture = Fixture::new(Buses::Both).await;
    fixture.settings.dbus.proxy = true;
    for (marker, what) in [("fail-close", "an unclean end"), ("no-ready", "no ready byte")] {
        fixture.fake.set(marker);

        fixture.service.probe(&fixture.settings).await;

        let status = fixture.service.current();
        assert_eq!(
            (status.bus_proxy, status.bus_proxy_off),
            (false, Some(BusProxyOff::SelfTest)),
            "{what}"
        );
        fixture.fake.unset(marker);
    }
    fixture.service.probe(&fixture.settings).await;
    assert!(fixture.service.current().bus_proxy, "a passed self-test turns it on again");
}

#[tokio::test]
async fn bus_proxy_off_below_the_version_floor() {
    let mut fixture = Fixture::new(Buses::Both).await;
    fixture.settings.dbus.proxy = true;
    for (version, off) in [
        ("xdg-dbus-proxy 0.1.4", Some(BusProxyOff::OldVersion)),
        ("xdg-dbus-proxy", Some(BusProxyOff::OldVersion)),
        ("xdg-dbus-proxy 0.1.5", Some(BusProxyOff::OldVersion)),
        ("xdg-dbus-proxy 0.1.8", Some(BusProxyOff::OldVersion)),
        ("xdg-dbus-proxy 0.1.9", None),
        ("xdg-dbus-proxy 0.2", None),
    ] {
        fixture.fake.version(version);

        fixture.service.probe(&fixture.settings).await;

        assert_eq!(fixture.service.current().bus_proxy_off, off, "{version}");
    }
    // A distribution's older package is a warning only when the user set the key.
    fixture.fake.version("xdg-dbus-proxy 0.1.5");
    fixture.service.probe(&fixture.settings).await;
    fixture.settings.set_source("dbus.proxy", efr_config::Source::File);
    let reported = fixture.service.reported(&fixture.settings);
    assert!(
        reported.warnings.iter().any(|warning| warning.contains("older than 0.1.9")),
        "{:?}",
        reported.warnings
    );
    fixture.settings.set_source("dbus.proxy", efr_config::Source::Default);
    let reported = fixture.service.reported(&fixture.settings);
    assert_eq!(reported.bus_proxy_off, Some(BusProxyOff::OldVersion));
    assert!(reported.warnings.is_empty(), "{:?}", reported.warnings);
    assert_eq!(super::parse_version("xdg-dbus-proxy 0.1.9\n"), Some((0, 1, 9)));
    assert_eq!(super::parse_version("xdg-dbus-proxy 1.x"), None);
}

#[tokio::test]
async fn status_shows_bus_filter_off_reason() {
    let mut fixture = Fixture::new(Buses::Both).await;
    fixture.settings.dbus.proxy = true;
    fixture.fake.set("fail-close");
    fixture.service.probe(&fixture.settings).await;

    let on = fixture.service.reported(&fixture.settings);
    let mut off_settings = fixture.settings.clone();
    off_settings.dbus.proxy = false;
    let off = fixture.service.reported(&off_settings);

    assert_eq!((on.bus_proxy, on.bus_proxy_off), (false, Some(BusProxyOff::SelfTest)));
    let warning = super::off_warning(BusProxyOff::SelfTest);
    assert_eq!(on.warnings.iter().filter(|line| **line == warning).count(), 1);
    assert_eq!((off.bus_proxy, off.bus_proxy_off), (false, Some(BusProxyOff::Config)));
    assert!(!off.warnings.contains(&warning), "dbus.proxy = false is the user's choice");
}

#[tokio::test]
async fn reload_that_turns_dbus_proxy_on_runs_the_self_test_and_asks_for_the_engine() {
    let fixture = Fixture::new(Buses::Both).await;
    let mut follower = Follower::start(&fixture);
    assert_eq!(fixture.service.current().bus_proxy_off, Some(BusProxyOff::Config));

    let mut settings = fixture.settings.clone();
    settings.dbus.proxy = true;
    follower.settings.send_replace(Arc::new(settings));

    let request =
        Wait::new("a rebuild").until_some(|| follower.requests.try_recv().ok()).await.unwrap();
    assert!(matches!(request, crate::reload::Request::Rebuild { .. }), "{request:?}");
    assert!(fixture.service.current().bus_proxy);
    assert_eq!(fixture.fake.versions(), 1);
    request.answer(efr_protocol::AdminConfigReloadResult {
        applied: true,
        error: None,
        restart_needed: Vec::new(),
    });
    follower.stop().await;
}

/// `bus::follow` on a fixture's service, with the reload task's requests and the
/// settings it follows in the test's hands.
struct Follower {
    settings: tokio::sync::watch::Sender<Arc<Settings>>,
    requests: tokio::sync::mpsc::Receiver<crate::reload::Request>,
    _engine: tokio::sync::watch::Sender<Arc<Engine>>,
    stop: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Follower {
    fn start(fixture: &Fixture) -> Follower {
        let (settings, settings_rx) =
            tokio::sync::watch::channel(Arc::new(fixture.settings.clone()));
        let (engine, engine_rx) = tokio::sync::watch::channel(Arc::new(fixture.engine.clone()));
        let (reloads, requests) = crate::reload::Reloads::new();
        let stop = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(super::follow(
            fixture.service.clone(),
            reloads,
            settings_rx,
            fixture.service.status(),
            engine_rx,
            stop.clone(),
        ));
        Follower { settings, requests, _engine: engine, stop, task }
    }

    async fn stop(self) {
        self.stop.cancel();
        self.task.await.unwrap();
    }
}

#[tokio::test]
async fn bus_view_dropped_without_a_session_bus() {
    let fixture = Fixture::new(Buses::SystemOnly).await;
    let launch = Launch::Contained {
        grants: vec![
            Grant::BusFilter { bus: BusKind::System },
            Grant::BusFilter { bus: BusKind::Session },
        ],
    };

    let prepared = fixture.prepare(launch).await.unwrap();

    assert_eq!(prepared.notes, [NO_SESSION_BUS]);
    let spec = fixture.spec();
    assert!(spec.grants.contains(&Grant::BusFilter { bus: BusKind::System }));
    assert!(!spec.grants.contains(&Grant::BusFilter { bus: BusKind::Session }));
    let starts = fixture.fake.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0][3], "system.sock");
    prepared.bus.unwrap().finish().await;
}

#[tokio::test]
async fn bus_view_dropped_without_a_system_bus() {
    let fixture = Fixture::new(Buses::SessionOnly).await;
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };

    let prepared = fixture.prepare(launch).await.unwrap();

    assert_eq!(prepared.notes, [NO_SYSTEM_BUS]);
    assert!(prepared.bus.is_none(), "no proxy starts");
    assert!(fixture.spec().grants.is_empty(), "the call runs with no bus, as before");
    assert!(fixture.fake.starts().is_empty());
}

#[tokio::test]
async fn bus_proxy_off_in_test_build_without_seam() {
    // NOTE: the unit tests build the daemon with the test seams; without a seam the
    // system bus is a path that never exists, so no proxy starts for it, and a whole
    // bus grant names that path too.
    let fixture = Fixture::new(Buses::None).await;
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };

    let prepared = fixture.prepare(launch).await.unwrap();

    assert_eq!(prepared.notes, [NO_SYSTEM_BUS]);
    assert!(prepared.bus.is_none());
    assert!(fixture.fake.starts().is_empty());
    let system = fixture.spec().runtime.system_bus;
    assert_ne!(system, Path::new(efr_sandbox::SYSTEM_BUS_SOCKET));
    assert!(!system.exists());
}

#[tokio::test]
async fn bus_proxy_off_for_session_in_test_build_without_seam() {
    // NOTE: the shells' environment names a session bus that exists, as on a developer
    // machine; a test build never takes it.
    let fixture = Fixture::new(Buses::None).await;
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::Session }] };

    let prepared = fixture.prepare(launch).await.unwrap();

    assert_eq!(prepared.notes, [NO_SESSION_BUS]);
    assert!(prepared.bus.is_none());
    assert!(fixture.fake.starts().is_empty());
    assert_eq!(fixture.spec().runtime.session_bus, None);
}

#[tokio::test]
async fn bus_grant_uses_the_test_system_bus() {
    let fixture = Fixture::new(Buses::Both).await;
    let launch = Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::System }] };

    let prepared = fixture.prepare(launch).await.unwrap();

    let spec = fixture.spec();
    assert_eq!(spec.runtime.system_bus, fixture.fake.system);
    assert_eq!(spec.runtime.session_bus.as_deref(), Some(fixture.fake.session.as_path()));
    assert!(prepared.bus.is_none(), "a whole bus starts no filter");
}

#[tokio::test]
async fn bus_proxy_of_a_job_has_its_own_call_dir() {
    let fixture = Fixture::new(Buses::Both).await;
    let launch = Launch::Contained { grants: vec![Grant::BusFilter { bus: BusKind::System }] };
    let call = fixture.call(5, launch);
    let job = JobInput { plan: JobPlan::new(60, 19), line: "systemctl --failed", env: b"" };

    let prepared = fixture.service.prepare_job(&call, &fixture.input(), job).await.unwrap();

    let start = &fixture.fake.starts()[0];
    assert_eq!(start[0], prepared.run.dir.join("bus").to_string_lossy());
    assert!(prepared.run.dir.starts_with(fixture.dirs.runtime().join("sbx")));
    assert!(prepared.run.dir.parent().is_some_and(|dir| dir.ends_with("jobs")));
    prepared.bus.unwrap().finish().await;
}

/// Which stand-in buses the seams name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Buses {
    Both,
    SystemOnly,
    SessionOnly,
    None,
}

/// A sandbox service on temp dirs whose launcher is the fake's, with the fake as the
/// trusted `xdg-dbus-proxy` and the stand-in buses in the seams.
struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    dirs: Dirs,
    store: TestStore,
    fake: FakeBus,
    buses: Buses,
    service: SandboxService,
    settings: Settings,
    engine: Engine,
}

impl Fixture {
    async fn new(buses: Buses) -> Fixture {
        let (dir, root) = temp();
        let home = root.join("home");
        let dirs =
            Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r")).unwrap();
        for made in [&home.join("p/app"), &dirs.config().to_path_buf()] {
            std::fs::create_dir_all(made).unwrap();
        }
        let fake = FakeBus::new(&root.join("fake"));
        let report = ProbeReport {
            bwrap: Some("/usr/bin/bwrap".into()),
            landlock_abi: Some(10),
            ..ProbeReport::default()
        };
        std::fs::write(fake.dir().join("report.json"), report.to_json().unwrap()).unwrap();
        let store = TestStore::open(TestClock::new().shared()).await.unwrap();
        let locations = Locations::new(&home)
            .unwrap()
            .with_project(ProjectId::from_uuid(uuid::Uuid::from_u128(9)), home.join("p/app"))
            .unwrap();
        let engine = Engine::with_defaults(locations);
        // NOTE: no network, so no note of the network proxy's fallback; and no bus
        // filter, whatever the default: each test turns it on itself.
        let mut settings = Settings::default();
        settings.network.mode = efr_protocol::NetworkMode::None;
        settings.dbus.proxy = false;
        let service = service(&dirs, &home, &fake, &store, buses, &settings, fake.programs()).await;
        Fixture { _dir: dir, home, dirs, store, fake, buses, service, settings, engine }
    }

    /// Another probed service on the same dirs, with `programs`.
    async fn service_with(
        &self,
        programs: Arc<crate::trusted_programs::TrustedPrograms>,
    ) -> SandboxService {
        service(
            &self.dirs,
            &self.home,
            &self.fake,
            &self.store,
            self.buses,
            &self.settings,
            programs,
        )
        .await
    }

    fn input(&self) -> PrepareInput<'_> {
        PrepareInput {
            settings: &self.settings,
            engine: &self.engine,
            named_paths: Vec::new(),
            mount_view: false,
        }
    }

    fn call(&self, n: u128, launch: Launch) -> CallContext {
        let id = |n: u128| uuid::Uuid::from_u128(n);
        CallContext::new(
            ConversationId::from_uuid(id(1)),
            TurnId::from_uuid(id(2)),
            CallId::from_uuid(id(n)),
            self.home.join("p/app"),
            self.dirs.data().join("scratch/x"),
        )
        .with_scope(Scope::Project(ProjectId::from_uuid(id(9))))
        .with_launch(launch)
    }

    async fn prepare(&self, launch: Launch) -> Result<crate::sandbox::Prepared, DaemonError> {
        self.prepare_n(3, launch).await
    }

    async fn prepare_n(
        &self,
        n: u128,
        launch: Launch,
    ) -> Result<crate::sandbox::Prepared, DaemonError> {
        self.service.prepare(&self.call(n, launch), &self.input()).await
    }

    async fn prepare_on(
        &self,
        service: &SandboxService,
        launch: Launch,
    ) -> Result<crate::sandbox::Prepared, DaemonError> {
        service.prepare(&self.call(6, launch), &self.input()).await
    }

    /// The call dir of call 3.
    fn call_dir(&self) -> PathBuf {
        let conversation = ConversationId::from_uuid(uuid::Uuid::from_u128(1));
        let call = CallId::from_uuid(uuid::Uuid::from_u128(3));
        self.dirs.runtime().join("sbx").join(conversation.to_string()).join(call.to_string())
    }

    /// The spec of call 3.
    fn spec(&self) -> SandboxSpec {
        SandboxSpec::from_json(&std::fs::read(self.call_dir().join("spec.json")).unwrap()).unwrap()
    }
}

/// A probed service on `dirs` whose seams name the stand-in buses of `buses`.
async fn service(
    dirs: &Dirs,
    home: &Path,
    fake: &FakeBus,
    store: &TestStore,
    buses: Buses,
    settings: &Settings,
    programs: Arc<crate::trusted_programs::TrustedPrograms>,
) -> SandboxService {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let mut deps = Deps::new(dirs.clone(), home, Arc::clone(&clock), Arc::new(TestRng::new(3)));
    if matches!(buses, Buses::Both | Buses::SystemOnly) {
        deps = deps.with_system_bus(&fake.system);
    }
    if matches!(buses, Buses::Both | Buses::SessionOnly) {
        deps = deps.with_session_bus(&fake.session);
    }
    let service = SandboxService::new(parts(dirs, home, fake, store, deps.seams, programs)).await;
    service.probe(settings).await;
    service
}

fn parts(
    dirs: &Dirs,
    home: &Path,
    fake: &FakeBus,
    store: &TestStore,
    seams: Seams,
    programs: Arc<crate::trusted_programs::TrustedPrograms>,
) -> ServiceParts {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // NOTE: the shells' environment names a session bus that exists, as on a developer
    // machine: only the seams may give it to a test build.
    let host = HostFacts {
        path: "/usr/bin:/bin".to_owned(),
        user_runtime: fake.dir().join("xrt"),
        session_bus: Some(fake.session.clone()),
        ..HostFacts::default()
    };
    ServiceParts {
        dirs: dirs.clone(),
        home: Home::new(home).unwrap(),
        host,
        source: Some(fake.launcher.clone()),
        seams,
        registry: dirs.config().join("projects.toml"),
        writer: store.writer().clone(),
        clock: Arc::clone(&clock),
        rng: Arc::new(TestRng::new(3)),
        git: Git::new(clock).isolated(),
        programs,
    }
}

/// The real launcher's `dbus-proxy` wrapper and the real `xdg-dbus-proxy` (checks C2-1
/// and C4-1 of the phase E plan): the self-test passes, the socket's name is relative
/// to the bus dir, a client of the filtered socket reaches the stand-in bus, and the
/// proxy ends cleanly and removes its socket when its sync pipe closes. It runs under
/// `just test-sandbox`, which names the launcher; a machine without `xdg-dbus-proxy`
/// has nothing to check.
#[tokio::test]
#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
async fn sandbox_real_wrapper_runs_the_real_proxy() {
    use std::os::unix::fs::FileTypeExt as _;
    let Ok(Some(launcher)) = efr_stdx::env::var(efr_stdx::env::Var::TestSbxBin) else {
        eprintln!("skipped: EFR_TEST_SBX_BIN is not set");
        return;
    };
    let programs = crate::trusted_programs::TrustedPrograms::resolve("/usr/bin".as_ref(), &[]);
    if !programs.has(crate::trusted_programs::Program::XdgDbusProxy) {
        eprintln!("note: /usr/bin/xdg-dbus-proxy is not installed; nothing to check");
        return;
    }
    let (_temp, root) = temp();
    let service = super::BusService::new(system_clock(), Arc::new(programs), launcher.into());
    service.self_test(&root.join("self-test")).await.unwrap();
    assert!(!root.join("self-test").exists(), "the self-test's dir goes");

    let upstream = root.join("bus.sock");
    let listener = UnixListener::bind(&upstream).unwrap();
    listener.set_nonblocking(true).unwrap();
    let call_dir = root.join("a".repeat(60)).join("b".repeat(60)).join("call");
    std::fs::create_dir_all(&call_dir).unwrap();
    let accepted = BusUpstream { system: Some(upstream.clone()), session: None };
    let call = service.start(&call_dir, &[BusKind::System], &accepted).await.unwrap();

    let socket = call_dir.join("bus/system.sock");
    assert!(std::fs::symlink_metadata(&socket).unwrap().file_type().is_socket());
    // NOTE: the socket's own path is longer than a socket address may be; the client
    // connects by its name from the bus dir, as the sandbox's bind does not need to.
    std::env::set_current_dir(call_dir.join("bus")).unwrap();
    let _client = std::os::unix::net::UnixStream::connect("system.sock").unwrap();
    Wait::new("the proxy's connection to the stand-in bus")
        .until(|| listener.accept().is_ok())
        .await
        .unwrap();
    let report = call.finish().await;
    assert!(!report.stopped);
    assert!(!socket.exists(), "the proxy removed its socket at its end");
}
