//! The filtered bus (phase E, efr's auto spec 11.1 and 16.7) end to end: efrd's own
//! probe, engine and bus filter, the real `efr-sbx` of `just test-sandbox`
//! (`EFR_TEST_SBX_BIN`) with its `dbus-proxy` wrapper, the real `xdg-dbus-proxy`, a real
//! hidden zsh (`EFR_TEST_ZSH=1`), and the real clients `hostnamectl`, `systemctl`,
//! `loginctl` and `busctl`.
//!
//! No test reaches a bus of the machine. Each test starts two private `dbus-daemon`
//! buses below its own dir and the fake services of `dbus/fake_bus.py` on them
//! ([`FakeBuses`]), and gives them to efrd through the seams of a test build
//! (`TestDaemon::bus_proxy`). In such a build both buses come only from the seams, also
//! for an approved whole bus. The fake logs each method call that reaches it, so a test
//! asserts the fake's log and the exit status, never a client's error text, and prints
//! the log when it fails. A call that the filter must refuse hides in a script that efr
//! does not read, on a line that also holds a read form, so the line gets the filtered
//! bus: `systemctl --failed --no-pager; ./start.sh`.
//!
//! The tests say `skipped:` without the launcher, zsh, a client, `xdg-dbus-proxy`,
//! `dbus-daemon` or Python's Gio, or when the probe says that the sandbox is
//! unavailable; `just test-sandbox` fails on such a line on a ready machine. Their dirs
//! stay below the target dir, not below /tmp, which the sandbox replaces with its own.

use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AdminStatus, AdminStatusResult, ApprovalDecision, BusKind, BusRefusal, Event, EventEnvelope,
    Grant, Launch, Method, Mode, SandboxSummary,
};
use efr_provider::Provider;
use efr_test_daemon::TestDaemon;
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::netview::{assert_routine, launches, outputs, target_dirs};
use super::{REVIEWER_ALLOWS, ScriptedModel, kinds, real_launcher, run_turn, skip, with_zsh};
use crate::support::zsh_enabled;

/// The fake services, written to each test's dir and run with `python3 -I`.
const FAKE_BUS: &str = include_str!("dbus/fake_bus.py");

/// How long a test waits for a bus, the fake or a process of the VM.
const SLOW: Duration = Duration::from_secs(45);

/// The programs that every test needs, each in `/usr/bin` or `/bin`.
const TOOLS: [&str; 7] =
    ["xdg-dbus-proxy", "dbus-daemon", "python3", "hostnamectl", "systemctl", "loginctl", "busctl"];

/// The file `tool` in `/usr/bin` or `/bin`, the dirs of the hidden shell's `PATH`.
fn find(tool: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin"].iter().map(|dir| Path::new(dir).join(tool)).find(|file| file.is_file())
}

/// One call that reached the fake.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    bus: String,
    name: String,
    path: String,
    member: String,
    /// False when the fake had no answer (`UnknownMethod`).
    answered: bool,
}

/// Two private `dbus-daemon` buses and the fake services on them. Drop kills them.
struct FakeBuses {
    system: PathBuf,
    session: PathBuf,
    log: PathBuf,
    _processes: Vec<tokio::process::Child>,
}

impl FakeBuses {
    /// Starts the buses in `dir/b` and the fake, and waits until the fake owns its
    /// names. The fake's `HOME` is `home`. `None` when `python3` cannot import Gio.
    async fn start(dir: &Path, home: &Path) -> Option<FakeBuses> {
        let gio = efr_stdx::process::command("python3", dir)
            .args(["-I", "-c", "from gi.repository import Gio, GLib"])
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        if !gio.status.success() {
            return None;
        }
        // NOTE: short names, so each socket's address fits in 107 bytes in CI too.
        let root = dir.join("b");
        std::fs::create_dir_all(&root).unwrap();
        let mut processes = Vec::new();
        let mut sockets = Vec::new();
        for name in ["sys", "ses"] {
            let socket = root.join(name);
            let config = root.join(format!("{name}.conf"));
            std::fs::write(&config, bus_config(&socket)).unwrap();
            let daemon = efr_stdx::process::command("dbus-daemon", &root)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--nopidfile"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(root.join(format!("{name}.err"))).unwrap())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            processes.push(daemon);
            sockets.push(socket);
        }
        for socket in &sockets {
            Wait::new("a private test bus")
                .limit(SLOW)
                .until(|| {
                    std::fs::symlink_metadata(socket).is_ok_and(|meta| meta.file_type().is_socket())
                })
                .await
                .unwrap();
        }
        let script = root.join("fake_bus.py");
        std::fs::write(&script, FAKE_BUS).unwrap();
        let log = root.join("fake.ndjson");
        std::fs::write(&log, "").unwrap();
        let [system, session] = <[PathBuf; 2]>::try_from(sockets).unwrap();
        let fake = efr_stdx::process::command("python3", &root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .arg("-I")
            .arg(&script)
            .arg(format!("unix:path={}", system.display()))
            .arg(format!("unix:path={}", session.display()))
            .arg(&log)
            .arg(home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(root.join("fake.err")).unwrap())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        processes.push(fake);
        let ready = || std::fs::read_to_string(&log).is_ok_and(|text| text.contains("\"ready\""));
        if let Err(error) = Wait::new("the fake bus services").limit(SLOW).until(ready).await {
            let err = std::fs::read_to_string(root.join("fake.err")).unwrap_or_default();
            panic!("{error}: {err}");
        }
        Some(FakeBuses { system, session, log, _processes: processes })
    }

    /// Every method call that reached the fake, in order.
    fn calls(&self) -> Vec<Call> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|entry| {
                let text = |key: &str| entry.get(key)?.as_str().map(str::to_owned);
                Some(Call {
                    bus: text("bus")?,
                    name: text("name")?,
                    path: text("path")?,
                    member: text("member")?,
                    answered: text("answer")? == "ok",
                })
            })
            .collect()
    }

    /// The members of the calls that reached the fake on `bus`.
    fn members(&self, bus: &str) -> Vec<String> {
        self.calls().into_iter().filter(|call| call.bus == bus).map(|call| call.member).collect()
    }

    /// The fake's log, for a failed assertion.
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

/// The config of a private test bus that listens on `socket`.
fn bus_config(socket: &Path) -> String {
    format!(
        "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\"\n \
         \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
         <busconfig>\n <type>session</type>\n <listen>unix:path={}</listen>\n \
         <auth>EXTERNAL</auth>\n <policy context=\"default\">\n  \
         <allow send_destination=\"*\" eavesdrop=\"true\"/>\n  <allow eavesdrop=\"true\"/>\n  \
         <allow own=\"*\"/>\n </policy>\n</busconfig>\n",
        socket.display()
    )
}

/// A daemon of a `dbus` test and its fake buses.
struct BusTest {
    daemon: TestDaemon,
    buses: FakeBuses,
    dirs: Arc<TestDirs>,
}

/// A daemon over `model` with the real launcher, efrd's own probe, a real hidden zsh,
/// the bus filter on with the real `xdg-dbus-proxy`, and two fake buses through the
/// seams. `None` after a `skipped:` line without zsh, the launcher or a tool, or when
/// the probe says that the sandbox is unavailable. The filter's self-test must pass
/// on a machine where the sandbox is ready.
async fn bus_daemon(test: &str, dirs: &Arc<TestDirs>, model: Arc<dyn Provider>) -> Option<BusTest> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    if let Some(tool) = TOOLS.iter().find(|tool| find(tool).is_none()) {
        skip(test, &format!("{tool} is not in /usr/bin or /bin"));
        return None;
    }
    let Some(buses) = FakeBuses::start(dirs.root(), dirs.home()).await else {
        skip(test, "python3 cannot import gi.repository.Gio (python-gobject, python3-gi)");
        return None;
    };
    let project = dirs.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
        ("SYSTEMD_PAGER".to_owned(), "cat".to_owned()),
        ("SYSTEMD_COLORS".to_owned(), "0".to_owned()),
    ]);
    // NOTE: a reload reads `config.toml` again over the built-in defaults, so the file
    // holds the filter's key too.
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::fs::write(
        dirs.dirs().config().join("config.toml"),
        "[permissions]\nmode = \"auto\"\n\n[dbus]\nproxy = true\n",
    )
    .unwrap();
    // NOTE: the clock starts after the files of the test were written, so no script of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let daemon = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .bus_proxy(Some(buses.system.clone()), Some(buses.session.clone()))
        .bus_proxy_program(find("xdg-dbus-proxy").unwrap())
        .config(|config| config.permissions.mode = Mode::Auto)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return None;
    }
    assert!(
        sandbox.bus_proxy,
        "the bus filter is off ({:?}): {:?}",
        sandbox.bus_proxy_off, sandbox.warnings
    );
    drop(client);
    Some(BusTest { daemon, buses, dirs: Arc::clone(dirs) })
}

/// Writes an executable script in the project, the hidden shell's working directory.
fn script(dirs: &TestDirs, name: &str, body: &str) {
    let path = dirs.home().join("project").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The sandbox summary and the exit code of each finished tool call of `events`.
fn summaries(events: &[EventEnvelope]) -> Vec<(Option<SandboxSummary>, Option<i32>)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { sandbox, exit_code, .. } => {
                Some((sandbox.clone(), *exit_code))
            }
            _ => None,
        })
        .collect()
}

/// The one call of `events`: its output, summary and exit code, with the fake's log in
/// the message of a failure.
fn one_call(events: &[EventEnvelope], buses: &FakeBuses) -> (String, SandboxSummary, Option<i32>) {
    let report = || format!("events: {:?}\nfake log:\n{}", kinds(events), buses.log());
    let [output] = outputs(events).try_into().unwrap_or_else(|_| panic!("{}", report()));
    let [(summary, code)] = summaries(events).try_into().unwrap_or_else(|_| panic!("{}", report()));
    let summary = summary.unwrap_or_else(|| panic!("no sandbox summary: {output}\n{}", report()));
    (output, summary, code)
}

/// The text of a failed assertion: the call's output, the filter's refusals and the
/// fake's log, which names a member that the defaults or the fake lack.
fn report(output: &str, summary: &SandboxSummary, buses: &FakeBuses) -> String {
    format!(
        "output:\n{output}\nrefused by the filter: {:?}\nfake log:\n{}",
        summary.bus_refused,
        buses.log()
    )
}

/// The launch of a read form of `bus`: contained, with the filtered bus only.
fn filtered(bus: BusKind) -> Launch {
    Launch::Contained { grants: vec![Grant::BusFilter { bus }] }
}

/// The refusal of the filter that the summary holds for one call.
fn refusal(bus: BusKind, name: &str, member: &str, path: &str) -> BusRefusal {
    BusRefusal {
        bus,
        name: name.to_owned(),
        member: member.to_owned(),
        path: path.to_owned(),
        count: 1,
    }
}

/// The pids of the processes whose command line holds `dbus-proxy` and the test's
/// root: the wrappers and the proxies of this test.
fn proxies(root: &Path) -> Vec<u32> {
    let root = root.display().to_string();
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let line = std::fs::read(entry.path().join("cmdline")).ok()?;
            let line = String::from_utf8_lossy(&line).replace('\0', " ");
            (line.contains("dbus-proxy") && line.contains(&root)).then_some(pid)
        })
        .collect()
}

/// Waits until no proxy of the test runs.
async fn no_proxy_left(root: &Path) {
    Wait::new("the end of the test's bus filters")
        .limit(SLOW)
        .until(|| proxies(root).is_empty())
        .await
        .unwrap_or_else(|error| panic!("{error}: {:?}", proxies(root)));
}

/// A turn over one call with `line` that runs routine: no exit, no question, no review.
/// Returns the call's output, summary and exit code.
async fn routine_line(
    test: &str,
    line: &str,
) -> Option<(String, SandboxSummary, Option<i32>, BusTest)> {
    let dirs = target_dirs();
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": line }))], REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let setup = bus_daemon(test, &dirs, provider).await?;
    let (_, events) = run_turn(&setup.daemon, "look", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    let (output, summary, code) = one_call(&events, &setup.buses);
    let report = report(&output, &summary, &setup.buses);
    let bus = if line.contains("--user") { BusKind::Session } else { BusKind::System };
    assert_eq!(launches(&events), [filtered(bus)], "{report}");
    Some((output, summary, code, setup))
}

#[tokio::test]
async fn shell_dbus_allows_hostnamectl() {
    let test = "shell_dbus_allows_hostnamectl";
    let Some((output, summary, code, setup)) = routine_line(test, "hostnamectl").await else {
        return;
    };
    let calls = setup.buses.calls();
    let report = report(&output, &summary, &setup.buses);
    assert_eq!(code, Some(0), "{report}");
    assert!(output.contains("efr-fake-host"), "{report}");
    assert!(!calls.is_empty(), "{report}");
    for call in &calls {
        assert_eq!(call.bus, "system", "{report}");
        assert_eq!(call.member, "org.freedesktop.DBus.Properties.GetAll", "{report}");
        assert!(call.answered, "{report}");
    }
    // NOTE: hostnamectl of systemd 262 also asks for the product UUID and the serial;
    // polkit guards both, so the defaults refuse them, as the real bus does.
    let polkit =
        ["org.freedesktop.hostname1.GetProductUUID", "org.freedesktop.hostname1.GetHardwareSerial"];
    for refused in &summary.bus_refused {
        assert!(polkit.contains(&refused.member.as_str()), "{report}");
    }
    setup.daemon.stop().await.unwrap();
    no_proxy_left(setup.dirs.root()).await;
}

#[tokio::test]
async fn shell_dbus_allows_systemctl_failed() {
    let test = "shell_dbus_allows_systemctl_failed";
    let line = "systemctl --failed --no-pager";
    let Some((output, summary, code, setup)) = routine_line(test, line).await else { return };
    let report = report(&output, &summary, &setup.buses);
    assert_eq!(code, Some(0), "{report}");
    assert!(output.contains("fake-failed.service"), "{report}");
    assert_eq!(
        setup.buses.members("system"),
        ["org.freedesktop.systemd1.Manager.ListUnitsByPatterns"],
        "{report}"
    );
    assert!(setup.buses.members("session").is_empty(), "{report}");
    assert_eq!(summary.bus_refused, Vec::new(), "{report}");
    setup.daemon.stop().await.unwrap();
}

/// Corpus row 10: `systemctl --user` reaches the session filter through the child
/// shell's function, which unsets `XDG_RUNTIME_DIR` for it (DE10).
#[tokio::test]
async fn shell_dbus_user_failed_routine() {
    let test = "shell_dbus_user_failed_routine";
    let line = "systemctl --user --failed --no-pager";
    let Some((output, summary, code, setup)) = routine_line(test, line).await else { return };
    let report = report(&output, &summary, &setup.buses);
    assert_eq!(code, Some(0), "{report}");
    assert!(output.contains("fake-user-failed.service"), "{report}");
    assert_eq!(
        setup.buses.members("session"),
        ["org.freedesktop.systemd1.Manager.ListUnitsByPatterns"],
        "{report}"
    );
    assert!(setup.buses.members("system").is_empty(), "{report}");
    assert_eq!(summary.bus_refused, Vec::new(), "{report}");
    setup.daemon.stop().await.unwrap();
}

/// Corpus row 11.
#[tokio::test]
async fn shell_dbus_loginctl_routine() {
    let test = "shell_dbus_loginctl_routine";
    let line = "loginctl list-sessions --no-pager";
    let Some((output, summary, code, setup)) = routine_line(test, line).await else { return };
    let report = report(&output, &summary, &setup.buses);
    assert_eq!(code, Some(0), "{report}");
    assert!(output.contains("seat0"), "{report}");
    let calls = setup.buses.calls();
    assert!(!calls.is_empty(), "{report}");
    assert!(calls.iter().all(|call| call.bus == "system" && call.answered), "{report}");
    assert_eq!(summary.bus_refused, Vec::new(), "{report}");
    setup.daemon.stop().await.unwrap();
}

/// A turn over one call with `line` whose script part the filter refuses. Returns the
/// call's output, summary and exit code; the line ran with no question and no review.
async fn refused_line(
    test: &str,
    line: &str,
    scripts: &[(&str, &str)],
    bus: BusKind,
) -> Option<(String, SandboxSummary, Option<i32>, BusTest)> {
    let dirs = target_dirs();
    for (name, body) in scripts {
        script(&dirs, name, body);
    }
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": line }))], REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let setup = bus_daemon(test, &dirs, provider).await?;
    let (_, events) = run_turn(&setup.daemon, "do it", Mode::Auto, ApprovalDecision::Deny).await;
    let (output, summary, code) = one_call(&events, &setup.buses);
    let report = report(&output, &summary, &setup.buses);
    assert_routine(&events, model.reviews());
    assert_eq!(launches(&events), [filtered(bus)], "{report}");
    assert!(code.is_some_and(|code| code != 0), "the script part fails: {code:?}\n{report}");
    Some((output, summary, code, setup))
}

/// The script that starts a unit on the system bus, which the filter refuses.
const START: &str = "busctl call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
                     org.freedesktop.systemd1.Manager StartUnit ss x.service replace";

#[tokio::test]
async fn shell_dbus_denies_startunit() {
    let test = "shell_dbus_denies_startunit";
    let line = "systemctl --failed --no-pager; ./start.sh";
    let scripts = [("start.sh", START)];
    let Some((output, summary, _, setup)) =
        refused_line(test, line, &scripts, BusKind::System).await
    else {
        return;
    };
    let report = report(&output, &summary, &setup.buses);
    assert!(output.contains("fake-failed.service"), "the read form ran: {report}");
    assert_eq!(
        setup.buses.members("system"),
        ["org.freedesktop.systemd1.Manager.ListUnitsByPatterns"],
        "no StartUnit reached the service: {report}"
    );
    assert_eq!(
        summary.bus_refused,
        [refusal(
            BusKind::System,
            "org.freedesktop.systemd1",
            "org.freedesktop.systemd1.Manager.StartUnit",
            "/org/freedesktop/systemd1"
        )],
        "{report}"
    );
    assert!(output.contains("[efr: the bus filter refused a call: bus=system"), "{report}");
    setup.daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_dbus_denies_user_enable_unit_files() {
    let test = "shell_dbus_denies_user_enable_unit_files";
    let line = "systemctl --user --failed --no-pager; ./enable.sh";
    let enable = "busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
                  org.freedesktop.systemd1.Manager EnableUnitFiles asbb 1 x.service false false";
    let Some((output, summary, _, setup)) =
        refused_line(test, line, &[("enable.sh", enable)], BusKind::Session).await
    else {
        return;
    };
    let report = report(&output, &summary, &setup.buses);
    assert_eq!(
        setup.buses.members("session"),
        ["org.freedesktop.systemd1.Manager.ListUnitsByPatterns"],
        "no EnableUnitFiles reached the user manager: {report}"
    );
    let units = setup.dirs.home().join(".config/systemd/user");
    assert!(!units.exists(), "no link in the home's unit dirs: {report}");
    assert_eq!(
        summary.bus_refused,
        [refusal(
            BusKind::Session,
            "org.freedesktop.systemd1",
            "org.freedesktop.systemd1.Manager.EnableUnitFiles",
            "/org/freedesktop/systemd1"
        )],
        "{report}"
    );
    setup.daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_dbus_denies_notifications() {
    let test = "shell_dbus_denies_notifications";
    let line = "systemctl --user --failed --no-pager; ./notify.sh";
    let notify = "busctl --user call org.freedesktop.Notifications /org/freedesktop/Notifications \
                  org.freedesktop.Notifications Notify susssasa{sv}i efr 0 '' hi there 0 0 5000";
    let Some((output, summary, _, setup)) =
        refused_line(test, line, &[("notify.sh", notify)], BusKind::Session).await
    else {
        return;
    };
    let report = report(&output, &summary, &setup.buses);
    let notifications: Vec<Call> = setup
        .buses
        .calls()
        .into_iter()
        .filter(|call| call.name == "org.freedesktop.Notifications")
        .collect();
    assert_eq!(notifications, Vec::new(), "{report}");
    assert_eq!(
        summary.bus_refused,
        [refusal(
            BusKind::Session,
            "org.freedesktop.Notifications",
            "org.freedesktop.Notifications.Notify",
            "/org/freedesktop/Notifications"
        )],
        "{report}"
    );
    setup.daemon.stop().await.unwrap();
}

/// After a refusal the model asks for the whole bus (`needs.bus`); the reviewer allows
/// it, and that one call reaches the test's own system bus, never the machine's. The
/// next filtered call is refused again.
#[tokio::test]
async fn shell_dbus_bus_exit_runs_real_bus_once() {
    let test = "shell_dbus_bus_exit_runs_real_bus_once";
    let dirs = target_dirs();
    script(&dirs, "start.sh", START);
    let filtered_line = json!({ "command": "systemctl --failed --no-pager; ./start.sh" });
    let calls = vec![
        ("shell", filtered_line.clone()),
        ("shell", json!({ "command": "./start.sh", "needs": { "bus": "system" } })),
        ("shell", filtered_line),
    ];
    let model = ScriptedModel::reviewed(calls, REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(setup) = bus_daemon(test, &dirs, provider).await else { return };
    let (_, events) =
        run_turn(&setup.daemon, "start x if it failed", Mode::Auto, ApprovalDecision::Deny).await;
    let report = format!("events: {:?}\nfake log:\n{}", kinds(&events), setup.buses.log());
    assert_eq!(model.reviews(), 1, "the reviewer judged the whole bus once: {report}");
    assert_eq!(
        launches(&events),
        [
            filtered(BusKind::System),
            Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::System }] },
            filtered(BusKind::System),
        ],
        "{report}"
    );
    let starts: Vec<Call> = setup
        .buses
        .calls()
        .into_iter()
        .filter(|call| call.member == "org.freedesktop.systemd1.Manager.StartUnit")
        .collect();
    assert_eq!(
        starts,
        [Call {
            bus: "system".to_owned(),
            name: "org.freedesktop.systemd1".to_owned(),
            path: "/org/freedesktop/systemd1".to_owned(),
            member: "org.freedesktop.systemd1.Manager.StartUnit".to_owned(),
            answered: true,
        }],
        "only the approved call reached the bus: {report}"
    );
    let codes: Vec<Option<i32>> = summaries(&events).into_iter().map(|(_, code)| code).collect();
    assert_eq!(codes[1], Some(0), "{report}");
    let refused: Vec<usize> = summaries(&events)
        .into_iter()
        .map(|(summary, _)| summary.map_or(0, |summary| summary.bus_refused.len()))
        .collect();
    assert_eq!(refused, [1, 0, 1], "{report}");
    setup.daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_dbus_proxy_gone_after_call() {
    let test = "shell_dbus_proxy_gone_after_call";
    let dirs = target_dirs();
    let line = "systemctl --failed --no-pager; systemctl --user --failed --no-pager";
    let model =
        ScriptedModel::reviewed(vec![("shell", json!({ "command": line }))], REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(setup) = bus_daemon(test, &dirs, provider).await else { return };
    let (_, events) = run_turn(&setup.daemon, "look", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    let report = format!("events: {:?}\nfake log:\n{}", kinds(&events), setup.buses.log());
    let launches = launches(&events);
    let [Launch::Contained { grants }] = launches.as_slice() else { panic!("{report}") };
    assert_eq!(
        *grants,
        [Grant::BusFilter { bus: BusKind::System }, Grant::BusFilter { bus: BusKind::Session }],
        "{report}"
    );
    assert_eq!(setup.buses.members("system").len(), 1, "{report}");
    assert_eq!(setup.buses.members("session").len(), 1, "{report}");
    // Both filters of the call ended with it, while efrd still runs.
    no_proxy_left(setup.dirs.root()).await;
    setup.daemon.stop().await.unwrap();
}

/// DE12: a background job whose line holds a read form gets its own filtered bus for
/// its whole life.
#[tokio::test]
async fn shell_dbus_job_reads_the_bus() {
    let test = "shell_dbus_job_reads_the_bus";
    let dirs = target_dirs();
    let job = json!({
        "command": "systemctl --failed --no-pager",
        "background": true,
        "max_minutes": 1,
    });
    let model = ScriptedModel::reviewed(vec![("shell", job)], REVIEWER_ALLOWS);
    let provider = Arc::clone(&model) as Arc<dyn Provider>;
    let Some(setup) = bus_daemon(test, &dirs, provider).await else { return };
    let (sent, events) =
        run_turn(&setup.daemon, "watch the units", Mode::Auto, ApprovalDecision::Deny).await;
    assert_routine(&events, model.reviews());
    let launch = events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, .. } => Some(job.launch.clone()),
        _ => None,
    });
    let report = format!("events: {:?}\nfake log:\n{}", kinds(&events), setup.buses.log());
    assert_eq!(launch, Some(filtered(BusKind::System)), "{report}");
    let client = setup.daemon.client().await.unwrap();
    let mut follow = setup.daemon.follow(&client, sent.conversation_id).await.unwrap();
    let ended =
        efr_test_daemon::events_until(&mut follow, |event| matches!(event, Event::JobEnded { .. }))
            .await
            .unwrap();
    let Some(Event::JobEnded { exit_code, bus_refused, .. }) =
        ended.last().map(|envelope| envelope.event.clone())
    else {
        unreachable!()
    };
    let report = format!("fake log:\n{}", setup.buses.log());
    assert_eq!(exit_code, Some(0), "{report}");
    assert_eq!(bus_refused, Vec::new(), "{report}");
    assert_eq!(
        setup.buses.members("system"),
        ["org.freedesktop.systemd1.Manager.ListUnitsByPatterns"],
        "{report}"
    );
    no_proxy_left(setup.dirs.root()).await;
    drop(client);
    setup.daemon.stop().await.unwrap();
}
