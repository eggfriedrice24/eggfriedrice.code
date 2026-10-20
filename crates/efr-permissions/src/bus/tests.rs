use efr_protocol::BusKind;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;
use serde::Deserialize;

use super::{BusFilter, BusRule, DEFAULT_NAMES, DEFAULT_RULES, check_rules};
use crate::{BusRuleProblem, PermissionsError};

const RECORDING: &str = include_str!("../../tests/fixtures/bus-recording.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recording {
    call: Vec<RecordedCall>,
    kept: Vec<Kept>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedCall {
    tool: String,
    systemd: u32,
    bus: BusKind,
    name: String,
    member: String,
    path: String,
    allow: bool,
    why: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    bus: BusKind,
    name: String,
    call: String,
    why: String,
}

fn rule(bus: BusKind, name: &str, calls: &[&str]) -> BusRule {
    BusRule {
        bus,
        name: name.to_owned(),
        call: calls.iter().map(|call| (*call).to_owned()).collect(),
    }
}

fn problem(rules: &[BusRule]) -> Option<BusRuleProblem> {
    match check_rules(rules) {
        Ok(()) => None,
        Err(PermissionsError::BusRule { problem, .. }) => Some(problem),
        Err(other) => panic!("not a bus rule error: {other}"),
    }
}

fn defaults(bus: BusKind) -> BusFilter {
    BusFilter::compile(bus, &[]).unwrap()
}

#[test]
fn bus_defaults_match_the_recording() {
    let recording: Recording = toml::from_str(RECORDING).unwrap();
    assert!(!recording.call.is_empty());
    for call in &recording.call {
        assert!(call.systemd >= 255, "{}", call.tool);
        let allowed = defaults(call.bus).allows(&call.name, &call.member, &call.path);
        assert_eq!(
            allowed, call.allow,
            "{} {} {} at {}",
            call.tool, call.name, call.member, call.path
        );
        assert_eq!(call.why.is_some(), !call.allow, "{}: a refused call says why", call.tool);
    }
    // Every default rule is used by an allowed call or kept on purpose, and every kept
    // rule is a default rule.
    for row in DEFAULT_RULES {
        for call in row.calls {
            let filter =
                BusFilter { bus: row.bus, rules: vec![(row.name.to_owned(), (*call).to_owned())] };
            let used = recording.call.iter().any(|seen| {
                seen.allow
                    && seen.bus == row.bus
                    && filter.allows(&seen.name, &seen.member, &seen.path)
            });
            let kept = recording
                .kept
                .iter()
                .any(|kept| kept.bus == row.bus && kept.name == row.name && kept.call == *call);
            assert!(used != kept, "{:?} {} {call}: used {used}, kept {kept}", row.bus, row.name);
        }
    }
    for kept in &recording.kept {
        assert!(!kept.why.is_empty());
        let known = DEFAULT_RULES.iter().any(|row| {
            row.bus == kept.bus && row.name == kept.name && row.calls.contains(&kept.call.as_str())
        });
        assert!(known, "kept rule is not a default: {} {}", kept.name, kept.call);
    }
}

#[test]
fn bus_proxy_args_of_the_defaults() {
    let p = "org.freedesktop.DBus.Properties";
    let m = "org.freedesktop.systemd1.Manager";
    let l = "org.freedesktop.login1.Manager";
    let systemd = "org.freedesktop.systemd1";
    let mut system = vec![
        "--filter".to_owned(),
        "--log".to_owned(),
        format!("--call=org.freedesktop.hostname1={p}.Get@/org/freedesktop/hostname1"),
        format!("--call=org.freedesktop.hostname1={p}.GetAll@/org/freedesktop/hostname1"),
        format!("--call={systemd}={p}.Get@/org/freedesktop/systemd1"),
        format!("--call={systemd}={p}.GetAll@/org/freedesktop/systemd1"),
    ];
    let manager = [
        "ListUnits",
        "ListUnitsByPatterns",
        "ListUnitFiles",
        "ListUnitFilesByPatterns",
        "GetUnitFileState",
        "GetUnit",
        "GetUnitByPIDFD",
        "ListJobs",
    ];
    let manager_args: Vec<String> = manager
        .iter()
        .map(|member| format!("--call={systemd}={m}.{member}@/org/freedesktop/systemd1"))
        .collect();
    let unit_args = [
        format!("--call={systemd}={p}.Get@/org/freedesktop/systemd1/unit/*"),
        format!("--call={systemd}={p}.GetAll@/org/freedesktop/systemd1/unit/*"),
    ];
    system.extend(manager_args.iter().cloned());
    system.extend(unit_args.iter().cloned());
    let login = [
        "ListSessionsEx",
        "ListSessions",
        "ListUsers",
        "ListSeats",
        "GetSession",
        "GetUser",
        "GetSeat",
    ];
    for member in login {
        system.push(format!("--call=org.freedesktop.login1={l}.{member}@/org/freedesktop/login1"));
    }
    system.push(format!("--call=org.freedesktop.login1={p}.Get@/org/freedesktop/login1/*"));
    system.push(format!("--call=org.freedesktop.login1={p}.GetAll@/org/freedesktop/login1/*"));
    for service in ["timedate1", "locale1", "timesync1"] {
        for member in ["Get", "GetAll"] {
            system.push(format!(
                "--call=org.freedesktop.{service}={p}.{member}@/org/freedesktop/{service}"
            ));
        }
    }
    assert_eq!(defaults(BusKind::System).proxy_args(), system);

    let mut session = vec!["--filter".to_owned(), "--log".to_owned()];
    session.extend(manager_args);
    session.extend(unit_args);
    let n = "org.freedesktop.Notifications";
    for member in ["Notify", "GetServerInformation", "GetCapabilities"] {
        session.push(format!("--call={n}={n}.{member}@/org/freedesktop/Notifications"));
    }
    assert_eq!(defaults(BusKind::Session).proxy_args(), session);
}

#[test]
fn bus_filter_matches_paths_as_the_proxy_does() {
    let system = defaults(BusKind::System);
    let get_all = "org.freedesktop.DBus.Properties.GetAll";
    // `@/path/*` matches the path itself and every path below it.
    assert!(system.allows("org.freedesktop.login1", get_all, "/org/freedesktop/login1"));
    assert!(system.allows("org.freedesktop.login1", get_all, "/org/freedesktop/login1/user/self"));
    assert!(!system.allows("org.freedesktop.login1", get_all, "/org/freedesktop/login1x"));
    // An exact path matches that path only.
    assert!(!system.allows("org.freedesktop.hostname1", get_all, "/org/freedesktop/hostname1/x"));
    // The name, the member and the path must all match.
    assert!(!system.allows("org.freedesktop.timedate1", get_all, "/org/freedesktop/locale1"));
    assert!(!system.allows(
        "org.freedesktop.systemd1",
        "org.freedesktop.systemd1.Manager.StartUnit",
        "/org/freedesktop/systemd1"
    ));
    // The session bus has no Properties rule at the manager path.
    let session = defaults(BusKind::Session);
    assert!(!session.allows("org.freedesktop.systemd1", get_all, "/org/freedesktop/systemd1"));
    assert!(session.allows(
        "org.freedesktop.systemd1",
        get_all,
        "/org/freedesktop/systemd1/unit/a_2eservice"
    ));
    // A rule with no path matches every path.
    let extra = [rule(BusKind::Session, "org.example.Player", &["org.example.Player.GetTrack"])];
    let session = BusFilter::compile(BusKind::Session, &extra).unwrap();
    assert!(session.allows("org.example.Player", "org.example.Player.GetTrack", "/any/where"));
}

#[rstest]
#[case::bare_star(rule(BusKind::System, "org.example.Foo", &["*"]), BusRuleProblem::Glob)]
#[case::member_glob(
    rule(BusKind::System, "org.freedesktop.systemd1", &["org.freedesktop.systemd1.Manager.ListUnits*"]),
    BusRuleProblem::Glob
)]
#[case::name_glob(rule(BusKind::Session, "org.example.*", &["org.example.Foo.Get"]), BusRuleProblem::Glob)]
#[case::path_glob(rule(BusKind::Session, "org.example.Foo", &["org.example.Foo.Get@/a/*/b"]), BusRuleProblem::Glob)]
#[case::interface_wildcard(
    rule(BusKind::System, "org.freedesktop.systemd1", &["org.freedesktop.systemd1.Manager.*"]),
    BusRuleProblem::InterfaceWildcard
)]
#[case::path_only(
    rule(BusKind::System, "org.freedesktop.systemd1", &["@/org/freedesktop/systemd1"]),
    BusRuleProblem::PathOnly
)]
#[case::star_path(
    rule(BusKind::System, "org.freedesktop.systemd1", &["*@/org/freedesktop/systemd1/*"]),
    BusRuleProblem::PathOnly
)]
#[case::empty_call(rule(BusKind::System, "org.example.Foo", &[]), BusRuleProblem::EmptyCall)]
#[case::bad_name(rule(BusKind::System, "example", &["org.example.Foo.Get"]), BusRuleProblem::BadName)]
#[case::digit_name(rule(BusKind::System, "org.1example", &["org.example.Foo.Get"]), BusRuleProblem::BadName)]
#[case::unique_name(rule(BusKind::System, ":1.42", &["org.example.Foo.Get"]), BusRuleProblem::UniqueName)]
#[case::bus_driver(
    rule(BusKind::System, "org.freedesktop.DBus", &["org.freedesktop.DBus.ListNames"]),
    BusRuleProblem::BusDriver
)]
#[case::properties_set(
    rule(BusKind::Session, "org.example.Foo", &["org.freedesktop.DBus.Properties.Set@/org/example"]),
    BusRuleProblem::PropertiesSet
)]
#[case::properties_set_default(
    rule(BusKind::System, "org.freedesktop.timedate1", &["org.freedesktop.DBus.Properties.Set"]),
    BusRuleProblem::PropertiesSet
)]
#[case::no_interface(
    rule(BusKind::Session, "org.example.Foo", &["GetTrack"]),
    BusRuleProblem::BadCall { call: "GetTrack".to_owned() }
)]
#[case::bad_path(
    rule(BusKind::Session, "org.example.Foo", &["org.example.Foo.Get@relative"]),
    BusRuleProblem::BadCall { call: "org.example.Foo.Get@relative".to_owned() }
)]
#[case::trailing_slash(
    rule(BusKind::Session, "org.example.Foo", &["org.example.Foo.Get@/org/"]),
    BusRuleProblem::BadCall { call: "org.example.Foo.Get@/org/".to_owned() }
)]
fn bus_rules_refuse_globs_path_only_and_floors(
    #[case] entry: BusRule,
    #[case] expected: BusRuleProblem,
) {
    assert_eq!(problem(&[entry]), Some(expected));
}

#[rstest]
#[case("org.freedesktop.secrets")]
#[case("org.freedesktop.portal.Desktop")]
#[case("org.freedesktop.impl.portal.desktop.gtk")]
#[case("org.freedesktop.PolicyKit1")]
#[case("org.freedesktop.Flatpak")]
#[case("org.freedesktop.Flatpak.SystemHelper")]
#[case("org.kde.kwalletd5")]
#[case("org.kde.kwalletd6")]
#[case("ca.desrt.dconf")]
#[case("org.freedesktop.Secrets")]
fn bus_rules_refuse_the_floor_names(#[case] name: &str) {
    let entry = rule(BusKind::Session, name, &["org.example.Foo.GetThing"]);
    assert_eq!(problem(&[entry]), Some(BusRuleProblem::FloorName { name: name.to_owned() }));
}

#[test]
fn bus_rules_refuse_write_members_without_case() {
    // For a service outside the defaults the deny list holds, matched without case.
    for member in [
        "org.freedesktop.systemd1.Manager.KExec",
        "org.example.Power.kexec",
        "org.freedesktop.systemd1.Manager.UnsetEnvironment",
        "org.freedesktop.systemd1.Manager.TryRestartUnit",
        "org.freedesktop.login1.Manager.PowerOffWithFlags",
        "org.freedesktop.systemd1.Manager.SwitchRoot",
        "org.freedesktop.systemd1.Manager.EnqueueMarkedJobs",
        "org.freedesktop.hostname1.SetHostname",
        "org.freedesktop.login1.Session.Terminate",
        "org.mpris.MediaPlayer2.Player.STOP",
    ] {
        let entry = rule(BusKind::Session, "org.example.Other", &[member]);
        assert_eq!(
            problem(&[entry]),
            Some(BusRuleProblem::WriteMember { member: member.to_owned() }),
            "{member}"
        );
    }
    // The deny list is a floor, not a full guard: a member that starts with no word of
    // it is the user's choice.
    let entry = rule(
        BusKind::Session,
        "org.freedesktop.Notifications",
        &["org.freedesktop.Notifications.Notify", "org.freedesktop.Notifications.GetCapabilities"],
    );
    assert_eq!(problem(&[entry]), None);
    let flatpak = rule(
        BusKind::Session,
        "org.freedesktop.Flatpak",
        &["org.freedesktop.Flatpak.Development.HostCommand"],
    );
    assert_eq!(
        problem(&[flatpak]),
        Some(BusRuleProblem::FloorName { name: "org.freedesktop.Flatpak".to_owned() })
    );
}

#[test]
fn bus_rules_default_names_take_only_read_members() {
    for name in DEFAULT_NAMES {
        for member in [
            "org.freedesktop.systemd1.Manager.KExec",
            "org.freedesktop.systemd1.Manager.UnsetEnvironment",
            "org.freedesktop.systemd1.Manager.TryRestartUnit",
            "org.freedesktop.login1.Manager.PowerOffWithFlags",
            "org.freedesktop.systemd1.Manager.SwitchRoot",
            "org.freedesktop.systemd1.Manager.EnqueueMarkedJobs",
            "org.freedesktop.systemd1.Manager.AddDependencyUnitFiles",
            "org.freedesktop.hostname1.SetHostname",
            "org.freedesktop.login1.Session.Terminate",
            "org.freedesktop.systemd1.Job.Cancel",
            "org.freedesktop.systemd1.Manager.CancelJob",
            "org.freedesktop.systemd1.Manager.Getaway",
            "org.freedesktop.systemd1.Unit.Ref",
            "org.freedesktop.login1.Seat.SwitchTo",
            "org.freedesktop.DBus.Introspectable.Introspect",
        ] {
            let entry = rule(BusKind::System, name, &[member]);
            assert_eq!(
                problem(&[entry]),
                Some(BusRuleProblem::NotARead { member: member.to_owned() }),
                "{name} {member}"
            );
        }
        let reads = rule(
            BusKind::System,
            name,
            &[
                "org.freedesktop.DBus.Properties.Get@/org/freedesktop/x",
                "org.freedesktop.DBus.Properties.GetAll",
                "org.freedesktop.systemd1.Manager.GetUnitProcesses@/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager.ListUnitsFiltered",
                "org.freedesktop.login1.Manager.CanSuspend",
                "org.freedesktop.systemd1.Manager.LookupDynamicUserByName",
            ],
        );
        assert_eq!(problem(&[reads]), None, "{name}");
    }
    // The defaults themselves pass the check that a user entry passes.
    let rows: Vec<BusRule> =
        DEFAULT_RULES.iter().map(|row| rule(row.bus, row.name, row.calls)).collect();
    assert_eq!(check_rules(&rows), Ok(()));
}

#[test]
fn bus_rules_extra_add_to_defaults() {
    let extra = [
        rule(
            BusKind::Session,
            "org.freedesktop.Notifications",
            &["org.freedesktop.Notifications.GetServerInformation"],
        ),
        rule(
            BusKind::System,
            "org.freedesktop.systemd1",
            &[
                "org.freedesktop.systemd1.Manager.GetUnitProcesses@/org/freedesktop/systemd1",
                // A rule that the defaults already hold comes once.
                "org.freedesktop.systemd1.Manager.ListJobs@/org/freedesktop/systemd1",
            ],
        ),
    ];
    let system = BusFilter::compile(BusKind::System, &extra).unwrap();
    let base = defaults(BusKind::System).proxy_args();
    let mut expected = base.clone();
    expected.push(
        "--call=org.freedesktop.systemd1=org.freedesktop.systemd1.Manager.GetUnitProcesses@/org/freedesktop/systemd1"
            .to_owned(),
    );
    assert_eq!(system.proxy_args(), expected, "the session entry is skipped");
    assert_eq!(system.bus(), BusKind::System);
    let session = BusFilter::compile(BusKind::Session, &extra).unwrap();
    let args = session.proxy_args();
    assert_eq!(
        args.last().map(String::as_str),
        Some(
            "--call=org.freedesktop.Notifications=org.freedesktop.Notifications.GetServerInformation"
        )
    );
    assert_eq!(args.len(), defaults(BusKind::Session).proxy_args().len() + 1);

    // A refused entry fails the compile of either bus, with its index.
    let bad = [extra[0].clone(), rule(BusKind::Session, "org.example.Foo", &["*"])];
    let error = BusFilter::compile(BusKind::System, &bad).unwrap_err();
    assert_eq!(error, PermissionsError::BusRule { index: 1, problem: BusRuleProblem::Glob });
    assert_eq!(error.to_string(), "dbus.allow[1] is refused");
}

#[test]
fn bus_rule_parses_with_unknown_fields_refused() {
    let entry: BusRule = toml::from_str(
        "bus = \"system\"\nname = \"org.freedesktop.timedate1\"\ncall = [\"org.freedesktop.DBus.Properties.GetAll\"]\n",
    )
    .unwrap();
    assert_eq!(
        entry,
        rule(
            BusKind::System,
            "org.freedesktop.timedate1",
            &["org.freedesktop.DBus.Properties.GetAll"]
        )
    );
    let unknown = toml::from_str::<BusRule>(
        "bus = \"system\"\nname = \"org.example.Foo\"\ncall = []\ntalk = true\n",
    );
    assert!(unknown.is_err());
}

fn call_word() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("*".to_owned()),
        Just("@/org/example".to_owned()),
        Just("*@/org/example/*".to_owned()),
        Just("org.example.Foo.*".to_owned()),
        Just("org.freedesktop.DBus.Properties.Set".to_owned()),
        "[A-Za-z]{1,6}(\\.[A-Za-z]{1,6}){1,3}(@(/[a-z0-9_]{1,5}){0,3}(/\\*)?)?",
        "[a-zA-Z.*@/_-]{0,24}",
    ]
}

fn name_word() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("org.freedesktop.systemd1".to_owned()),
        Just("org.freedesktop.portal.Desktop".to_owned()),
        Just(":1.7".to_owned()),
        "[a-z]{1,6}(\\.[A-Za-z0-9_-]{1,6}){1,3}",
        "[a-zA-Z.*:_-]{0,16}",
    ]
}

fn bus_word() -> impl Strategy<Value = BusKind> {
    prop_oneof![Just(BusKind::System), Just(BusKind::Session)]
}

proptest! {
    #[test]
    fn bus_proxy_args_never_talk_own_see_or_broadcast(
        entries in proptest::collection::vec(
            (bus_word(), name_word(), proptest::collection::vec(call_word(), 0..4)),
            0..5,
        ),
        bus in bus_word(),
    ) {
        let extra: Vec<BusRule> = entries
            .into_iter()
            .map(|(bus, name, call)| BusRule { bus, name, call })
            .collect();
        if let Ok(filter) = BusFilter::compile(bus, &extra) {
            let args = filter.proxy_args();
            prop_assert_eq!(&args[..2], &["--filter".to_owned(), "--log".to_owned()]);
            for word in &args[2..] {
                for refused in ["--talk", "--own", "--see", "--broadcast"] {
                    prop_assert!(!word.starts_with(refused), "{}", word);
                }
                let rest = word.strip_prefix("--call=");
                prop_assert!(rest.is_some(), "{}", word);
                let (name, call) = rest.unwrap_or_default().split_once('=').unwrap_or_default();
                prop_assert!(!name.is_empty() && !name.contains('*'), "{}", word);
                let (method, path) = call.split_once('@').unwrap_or((call, ""));
                prop_assert!(!method.contains('*'), "{}", word);
                prop_assert!(method.contains('.') && !method.ends_with('.'), "{}", word);
                let base = path.strip_suffix("/*").unwrap_or(path);
                prop_assert!(!base.contains('*'), "{}", word);
            }
        }
    }
}
