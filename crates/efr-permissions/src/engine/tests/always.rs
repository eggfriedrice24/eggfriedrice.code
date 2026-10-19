//! Always-allow (phase E, efr's auto spec 11.2 and 11.3): the floor checks of a rule when
//! it loads, and the matcher that says whether a rule covers every exit of a call, on
//! the real paths of its grants (DE20 to DE25, DE44).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{ExitKind, Grant, Mode, Needs, Origin, ProjectId, Scope};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use crate::exits::WriteBind;
use crate::{
    Action, AllowFloor, AllowRule, AllowRuleProblem, AlwaysVerdict, Approver, AutoSupport,
    CallFacts, ConversationPolicy, Decision, DecisionInput, Effect, Egress, Engine, HostList,
    Locations, Policy, Requirements, Resource, Rule, TargetKind,
};

const HOME: &str = "/home/u";
const APP: &str = "/home/u/p/app";
const LIB: &str = "/home/u/p/lib";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-19-always-0a1b2c3d";
const DIR: Option<TargetKind> = Some(TargetKind::Dir);
const FILE: Option<TargetKind> = Some(TargetKind::File);

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// `~/p/app`, the turn's project.
fn app() -> ProjectId {
    id(1)
}

/// The machine: efr's config and dirs, a sealed secret root, the turn's project and a
/// second project that lines may name (an envelope root), the private tmp, a cache
/// overlay, a synced folder, a dotfile link floor and a `sandbox.protect` floor that
/// does not exist yet, and a credential agent from the environment.
fn machine() -> Locations {
    Locations::new(HOME)
        .unwrap()
        .with_sealed_root("/home/u/.local/share/efr/secrets")
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap()
        .with_efr_root(
            "/home/u/.local/share/efr",
            [PathBuf::from("/home/u/.local/share/efr/scratch")],
        )
        .unwrap()
        .with_efr_root("/home/u/.local/state/efr", [])
        .unwrap()
        .with_efr_root("/run/user/1000/efr", [])
        .unwrap()
        .with_project(app(), APP)
        .unwrap()
        .with_project(id(2), LIB)
        .unwrap()
        .with_envelope_root(LIB)
        .unwrap()
        .with_private_root("/tmp")
        .unwrap()
        .with_cache_root("/home/u/.cache")
        .unwrap()
        .with_synced_root("/home/u/Dropbox")
        .unwrap()
        .with_link_floor_root("/home/u/dotfiles/hypr", "/home/u/.config/hypr")
        .unwrap()
        .with_floor_root("/home/u/work/hooks")
        .unwrap()
        .with_credential_socket("/run/user/1000/vault.sock")
        .unwrap()
}

/// The host list of efr's proxy: one denied host.
fn hosts() -> HostList {
    HostList::compile(&[], &["evil.example.org".to_owned()], &[], &[]).unwrap()
}

/// The engine of [`machine`] with efr's proxy, the user's `rules` and the always
/// `allow` rules.
fn engine_with(rules: Vec<Rule>, allow: Vec<AllowRule>) -> Engine {
    let support =
        AutoSupport { egress: Egress::Proxy, floor_backup: true, ..AutoSupport::default() };
    Engine::with_rules(machine(), Policy::new(rules).unwrap())
        .with_support(support)
        .with_hosts(Arc::new(hosts()))
        .with_allow(allow)
}

fn engine(allow: Vec<AllowRule>) -> Engine {
    engine_with(Vec::new(), allow)
}

fn write(under: &str) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: None }
}

fn write_in(under: &str, project: &str) -> AllowRule {
    AllowRule::Write { under: under.to_owned(), project: Some(project.to_owned()) }
}

fn host(host: &str, port: u16) -> AllowRule {
    AllowRule::Host { host: host.to_owned(), port }
}

fn socket(path: &str) -> AllowRule {
    AllowRule::Socket { path: path.to_owned() }
}

/// A shell call of `line` in the project root.
fn shell(line: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(APP)
}

/// The facts of a call: the kind of each target, and the real path of each path.
fn facts(targets: &[(&str, Option<TargetKind>)], real: &[(&str, &str)]) -> CallFacts {
    CallFacts {
        targets: targets.iter().map(|(path, kind)| (PathBuf::from(path), *kind)).collect(),
        real_paths: real
            .iter()
            .map(|(path, real)| (PathBuf::from(path), PathBuf::from(real)))
            .collect(),
        ..CallFacts::default()
    }
}

/// Each path of `paths` as its own real path.
fn same(paths: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    paths.iter().map(|path| (*path, *path)).collect()
}

fn decide_as(engine: &Engine, requirements: Requirements, origin: Origin) -> Decision {
    engine.decide(&DecisionInput {
        requirements,
        scope: Scope::Project(app()),
        origin,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    })
}

/// The decision of `requirements` on `engine`, and the verdict of the always rules for
/// a turn of the project `~/p/app`. Every verdict passes [`assert_sound`].
fn judge(engine: &Engine, requirements: Requirements) -> (Decision, AlwaysVerdict) {
    let decision = decide_as(engine, requirements, Origin::Shell);
    let verdict = engine.always(&decision, Some(Path::new(APP)));
    assert_sound(engine, &decision, &verdict);
    (decision, verdict)
}

/// The rules of the matcher that hold for every decision: a covered or offered call
/// asks only for its exits, each a `host`, `write` or `socket` exit that the classifier
/// judges, runs in the sandbox and holds only narrow grants; a covering rule is enabled;
/// an offer passes the load check.
fn assert_sound(engine: &Engine, decision: &Decision, verdict: &AlwaysVerdict) {
    if *verdict == AlwaysVerdict::None {
        return;
    }
    assert_eq!(decision.effect(), Effect::Ask, "{verdict:?}");
    assert!(decision.exits().next().is_some(), "{verdict:?}");
    for need in decision.exits() {
        assert!(
            matches!(need.kind, ExitKind::Host | ExitKind::Write | ExitKind::Socket),
            "{need:?}"
        );
        assert_eq!(need.approver, Approver::Classifier, "{need:?}");
        assert!(!need.runs_unsandboxed(), "{need:?}");
        assert_ne!(need.bind, Some(WriteBind::ExitChild), "{need:?}");
        assert_ne!(need.source, efr_protocol::ExitSource::CacheDelete, "{need:?}");
        for grant in &need.grants {
            assert!(
                matches!(grant, Grant::Host { .. } | Grant::Write { .. } | Grant::Socket { .. }),
                "{grant:?}"
            );
        }
    }
    match verdict {
        AlwaysVerdict::Covered { index } => assert_eq!(engine.allow_rules()[*index].1, None),
        AlwaysVerdict::Offer(rule) => assert_eq!(engine.check_allow(rule), Ok(())),
        AlwaysVerdict::None => {}
    }
}

/// The disabled marks of `engine`'s rules, in order.
fn marks(engine: &Engine) -> Vec<Option<AllowFloor>> {
    engine.allow_rules().iter().map(|(_, disabled)| disabled.clone()).collect()
}

/// A copy to a new file `~/notes/a.txt` in the existing dir `~/notes`: the "yes" binds
/// the dir (`WriteBind::Parent`), because `cp` may make a temporary file there.
fn notes_write() -> Requirements {
    let file = "/home/u/notes/a.txt";
    shell("cp a.txt ~/notes/a.txt")
        .with_write(file)
        .with_facts(facts(&[(file, None), ("/home/u/notes", DIR)], &same(&[file, "/home/u/notes"])))
}

#[test]
fn always_covers_a_write_under_its_dir_and_offers_the_bound_dir() {
    let (decision, verdict) = judge(&engine(Vec::new()), notes_write());
    let need = decision.exits().next().unwrap();
    assert_eq!(need.grants, [Grant::Write { path: "/home/u/notes".into() }]);
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/notes", "~/p/app")));
    let engine = engine(vec![host("example.org", 443), write("~/notes")]);
    assert_eq!(marks(&engine), [None, None]);
    assert_eq!(judge(&engine, notes_write()).1, AlwaysVerdict::Covered { index: 1 });
    // A rule above the bound dir covers it too.
    assert_eq!(
        judge(&self::engine(vec![write("~/notes")]), notes_write()).1,
        AlwaysVerdict::Covered { index: 0 }
    );
}

#[rstest]
#[case::docker("/var/run/docker.sock")]
#[case::docker_rootless("/run/user/1000/docker.sock")]
#[case::podman("/run/user/1000/podman/podman.sock")]
#[case::podman_elsewhere("/home/u/podman.sock")]
#[case::user_manager("/run/user/1000/systemd/private")]
#[case::user_manager_other("/run/user/1000/systemd/notify")]
#[case::system_manager("/run/systemd/private")]
#[case::containerd("/run/containerd/containerd.sock")]
fn always_allow_rejects_privilege(#[case] path: &str) {
    let engine = engine(vec![socket(path)]);
    assert_eq!(marks(&engine), [Some(AllowFloor::ServiceSocket)], "{path}");
    // The exit of such a socket is a privilege exit, which no rule ever covers.
    let needs = Needs { sockets: vec![path.to_owned()], ..Needs::default() };
    let requirements = shell("make").with_needs(needs).with_facts(facts(&[], &[(path, path)]));
    let (decision, verdict) = judge(&engine, requirements);
    let name = Path::new(path).file_name().unwrap().to_str().unwrap();
    if crate::service_socket(path.as_ref()) || name.ends_with(".sock") {
        assert!(decision.exits().any(|need| need.kind == ExitKind::Privilege), "{path}");
    }
    assert_eq!(verdict, AlwaysVerdict::None, "{path}");
    // `sudo` and `docker` lines are privilege exits too.
    for line in ["sudo tee /home/u/notes/a.txt", "docker run --rm alpine"] {
        let (decision, verdict) = judge(&engine, shell(line));
        assert!(decision.exits().any(|need| need.kind == ExitKind::Privilege), "{line}");
        assert_eq!(verdict, AlwaysVerdict::None, "{line}");
    }
}

#[rstest]
#[case::zshrc("~/.zshrc", AllowFloor::UnderFloor { floor: "/home/u/.zshrc".into() })]
#[case::systemd("~/.config/systemd", AllowFloor::UnderFloor { floor: "/home/u/.config/systemd".into() })]
#[case::dbus("~/.local/share/dbus-1/services", AllowFloor::UnderFloor { floor: "/home/u/.local/share/dbus-1".into() })]
#[case::dotfile_target("~/dotfiles/hypr/x", AllowFloor::UnderFloor { floor: "/home/u/dotfiles/hypr".into() })]
#[case::config("~/.config/efr", AllowFloor::Config)]
#[case::projects_toml("~/.config/efr/projects.toml", AllowFloor::Config)]
#[case::data("~/.local/share/efr", AllowFloor::EfrDir)]
#[case::scratch("~/.local/share/efr/scratch", AllowFloor::EfrDir)]
#[case::state("~/.local/state/efr/logs", AllowFloor::EfrDir)]
#[case::runtime("/run/user/1000/efr", AllowFloor::OutsideHome)]
#[case::git_dir("~/notes/.git", AllowFloor::GitDir)]
#[case::git_hooks("~/notes/.git/hooks", AllowFloor::GitDir)]
#[case::above_home_floors("~/.config", AllowFloor::AboveHomeFloor { floor: "/home/u/.config/fish".into() })]
#[case::local_share("~/.local/share", AllowFloor::AboveHomeFloor { floor: "/home/u/.local/share/systemd".into() })]
#[case::home("~", AllowFloor::AtOrAboveHome)]
#[case::root("/", AllowFloor::AtOrAboveHome)]
#[case::system("/etc", AllowFloor::OutsideHome)]
#[case::secret("~/.ssh", AllowFloor::Secret)]
#[case::credential_mask("~/.aws", AllowFloor::Mask)]
#[case::dot_dot("~/notes/../.zshrc", AllowFloor::BadShape {
    problem: AllowRuleProblem::DotDot { path: "~/notes/../.zshrc".into() }
})]
#[case::mask("~/.zsh_history", AllowFloor::Mask)]
#[case::synced("~/Dropbox/notes", AllowFloor::Synced)]
#[case::write_root("~/p/lib", AllowFloor::WriteRoot { root: LIB.into() })]
#[case::above_a_write_root("~/p", AllowFloor::WriteRoot { root: LIB.into() })]
#[case::cache("~/.cache", AllowFloor::WriteRoot { root: "/home/u/.cache".into() })]
fn always_allow_rejects_floor_path(#[case] under: &str, #[case] floor: AllowFloor) {
    let engine = engine(vec![write("~/notes"), write(under)]);
    assert_eq!(marks(&engine), [None, Some(floor.clone())], "{under}");
    assert_eq!(engine.check_allow(&write(under)), Err(floor), "{under}");
}

#[test]
fn always_allow_rejects_a_dir_of_several_projects_and_keeps_bad_rules() {
    let locations = machine()
        .with_project(id(3), "/home/u/code/a")
        .unwrap()
        .with_project(id(4), "/home/u/code/b")
        .unwrap();
    let engine =
        Engine::with_defaults(locations).with_allow(vec![write("~/code"), write("~/code/a")]);
    assert_eq!(marks(&engine), [Some(AllowFloor::ManyProjects), None]);
    // A disabled rule stays in the list with its index, and the shape of a rule is
    // checked first.
    let engine = engine.with_allow(vec![write("notes"), write("~/notes")]);
    assert_eq!(engine.allow_rules().len(), 2);
    assert_eq!(
        marks(&engine)[0],
        Some(AllowFloor::BadShape { problem: AllowRuleProblem::BadPath { path: "notes".into() } })
    );
    assert_eq!(
        AllowFloor::UnderFloor { floor: "/home/u/.zshrc".into() }.to_string(),
        "it lies in /home/u/.zshrc, which runs code later"
    );
}

#[test]
fn always_allow_rejects_desktop_ipc() {
    // A read form of a desktop program is a `desktop_ipc` exit, which no rule covers.
    let engine = engine(vec![socket("/run/user/1000/hypr/abc/.socket.sock"), write("~/notes")]);
    assert_eq!(marks(&engine), [Some(AllowFloor::DesktopSocket), None]);
    let (decision, verdict) = judge(&engine, shell("hyprctl clients"));
    assert_eq!(decision.exits().next().unwrap().kind, ExitKind::DesktopIpc);
    assert_eq!(decision.exits().next().unwrap().approver, Approver::Classifier);
    assert_eq!(verdict, AlwaysVerdict::None);
    for path in
        ["/run/user/1000/wayland-1", "/tmp/.X11-unix/X0", "/run/user/1000/sway-ipc.1000.1.sock"]
    {
        assert_eq!(engine.check_allow(&socket(path)), Err(AllowFloor::DesktopSocket), "{path}");
    }
}

#[rstest]
#[case::system_bus("/run/dbus/system_bus_socket", AllowFloor::BusSocket)]
#[case::var_system_bus("/var/run/dbus/system_bus_socket", AllowFloor::BusSocket)]
#[case::session_bus("/run/user/1000/bus", AllowFloor::BusSocket)]
#[case::gpg("/run/user/1000/gnupg/S.gpg-agent", AllowFloor::CredentialAgent)]
#[case::ssh_agent("/run/user/1000/ssh-agent.socket", AllowFloor::CredentialAgent)]
#[case::vault("/run/user/1000/vault.sock", AllowFloor::CredentialAgent)]
#[case::in_project("/home/u/p/app/run/dev.sock", AllowFloor::InWriteRoot)]
#[case::in_named_project("/home/u/p/lib/dev.sock", AllowFloor::InWriteRoot)]
#[case::efrd("/run/user/1000/efr/daemon.sock", AllowFloor::EfrDir)]
#[case::secret("/home/u/.gnupg/S.gpg-agent", AllowFloor::Secret)]
fn always_socket_rule_floors(#[case] path: &str, #[case] floor: AllowFloor) {
    assert_eq!(engine(Vec::new()).check_allow(&socket(path)), Err(floor), "{path}");
}

#[test]
fn always_socket_rule_covers_one_socket() {
    let path = "/run/user/1000/mpd/socket";
    let needs = Needs { sockets: vec![path.to_owned()], ..Needs::default() };
    let requirements =
        || shell("mpc status").with_needs(needs.clone()).with_facts(facts(&[], &[(path, path)]));
    let (decision, verdict) = judge(&engine(Vec::new()), requirements());
    assert_eq!(decision.exits().next().unwrap().grants, [Grant::Socket { path: path.into() }]);
    assert_eq!(verdict, AlwaysVerdict::Offer(socket(path)));
    let engine = engine(vec![socket("/run/user/1000/other.sock"), socket(path)]);
    assert_eq!(judge(&engine, requirements()).1, AlwaysVerdict::Covered { index: 1 });
}

#[test]
fn always_socket_rule_never_covers_a_link_or_a_socket_in_a_write_root() {
    let engine =
        engine(vec![socket("/run/user/1000/mpd/socket"), socket("/home/u/p/app/dev.sock")]);
    assert_eq!(marks(&engine)[1], Some(AllowFloor::InWriteRoot));
    // A link: its real path differs, so no rule covers it, and none is offered.
    let path = "/run/user/1000/mpd/socket";
    let needs = Needs { sockets: vec![path.to_owned()], ..Needs::default() };
    let linked = shell("mpc status")
        .with_needs(needs)
        .with_facts(facts(&[], &[(path, "/run/user/1000/bus")]));
    assert_eq!(judge(&engine, linked).1, AlwaysVerdict::None);
    // A socket in a write root, which a contained call can replace with a link.
    let path = "/home/u/p/app/dev.sock";
    let needs = Needs { sockets: vec![path.to_owned()], ..Needs::default() };
    let in_root = shell("curl --unix-socket dev.sock http://x/")
        .with_needs(needs)
        .with_facts(facts(&[], &[(path, path)]));
    assert_eq!(judge(&engine, in_root).1, AlwaysVerdict::None);
}

#[test]
fn always_allow_never_unsandboxed() {
    let engine = engine(vec![write("~/notes"), host("github.com", 443)]);
    // A write that no contained bind serves runs in the exit child: a system path.
    let (decision, verdict) =
        judge(&engine, shell("echo x > /etc/hosts.d/x").with_write("/etc/hosts.d/x"));
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::ExitChild));
    assert_eq!(verdict, AlwaysVerdict::None);
    // A new file in a shared dir that the line's program does not make.
    let file = "/home/u/.config/new.conf";
    let requirements = shell("cp a.conf ~/.config/new.conf").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/.config", DIR)],
        &same(&[file, "/home/u/.config"]),
    ));
    let (decision, verdict) = judge(&engine, requirements);
    assert!(decision.exits().all(|need| need.runs_unsandboxed()));
    assert_eq!(verdict, AlwaysVerdict::None);
    // An `outside` exit and an upload.
    let needs = Needs { outside: true, ..Needs::default() };
    let (decision, verdict) = judge(&engine, shell("make").with_needs(needs));
    assert_eq!(decision.exits().next().unwrap().kind, ExitKind::Outside);
    assert_eq!(verdict, AlwaysVerdict::None);
    let (decision, verdict) = judge(&engine, shell("git push https://github.com/o/r.git main"));
    assert!(decision.exits().any(|need| need.kind == ExitKind::Upload));
    assert_eq!(verdict, AlwaysVerdict::None);
    // A write of the turn's project with the rule in place runs with no exit at all.
    let (decision, verdict) =
        judge(&engine, shell("echo x > out.txt").with_write("/home/u/p/app/out.txt"));
    assert_eq!(decision.effect(), Effect::Contain);
    assert_eq!(verdict, AlwaysVerdict::None);
}

#[test]
fn always_allow_ignored_for_phone_turn() {
    let engine = engine(vec![write("~/notes")]);
    let decision = decide_as(&engine, notes_write(), Origin::Phone);
    assert_eq!(decision.exits().count(), 0, "a phone turn runs as cautious: no exit");
    assert_eq!(engine.always(&decision, Some(Path::new(APP))), AlwaysVerdict::None);
    // The same call in a local turn is covered.
    assert_eq!(judge(&engine, notes_write()).1, AlwaysVerdict::Covered { index: 0 });
}

#[test]
fn deny_rule_beats_always_allow() {
    let allow = vec![write("~/notes"), host("download.example.org", 443)];
    let under =
        |dir: &str, effect: Effect| Rule::new(Action::Write, Resource::Under(dir.into()), effect);
    // A user `deny` of the written path: the call is denied.
    let engine = engine_with(vec![under("~/notes/a.txt", Effect::Deny)], allow.clone());
    let (decision, verdict) = judge(&engine, notes_write());
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(verdict, AlwaysVerdict::None);
    // A user `ask`: a reason that is no exit decides, so the user answers.
    let engine = engine_with(vec![under("~/notes", Effect::Ask)], allow.clone());
    let (decision, verdict) = judge(&engine, notes_write());
    assert_eq!(decision.effect(), Effect::Ask);
    assert_eq!(verdict, AlwaysVerdict::None);
    // A `deny` of a path that the line never names, below the bound dir `~/notes`.
    let engine = engine_with(vec![under("~/notes/private", Effect::Deny)], allow.clone());
    let (decision, verdict) = judge(&engine, notes_write());
    assert_eq!(decision.effect(), Effect::Ask);
    assert_eq!(verdict, AlwaysVerdict::None);
    // A `deny` elsewhere changes nothing.
    let engine = engine_with(vec![under("~/other", Effect::Deny)], allow);
    assert_eq!(judge(&engine, notes_write()).1, AlwaysVerdict::Covered { index: 0 });
    // `network.deny` disables a host rule; the proxy refuses the host anyway.
    let engine = self::engine(vec![host("evil.example.org", 443), host("EVIL.example.org.", 443)]);
    assert_eq!(marks(&engine), [Some(AllowFloor::NetworkDeny), Some(AllowFloor::NetworkDeny)]);
}

#[test]
fn always_allow_rejects_named_persistence_kind() {
    let engine = engine(vec![write("~/.config/systemd/user"), write("~/notes")]);
    assert_eq!(
        marks(&engine)[0],
        Some(AllowFloor::UnderFloor { floor: "/home/u/.config/systemd".into() })
    );
    let unit = "/home/u/.config/systemd/user/x.service";
    let requirements = shell("echo x > ~/.config/systemd/user/x.service")
        .with_write(unit)
        .with_facts(facts(&[(unit, FILE)], &same(&[unit, "/home/u/.config/systemd/user"])));
    let (decision, verdict) = judge(&engine, requirements);
    assert_eq!(decision.exits().next().unwrap().kind, ExitKind::Persistence);
    assert_eq!(verdict, AlwaysVerdict::None);
    let (decision, verdict) = judge(&engine, shell("systemctl --user enable --now x.service"));
    assert_eq!(decision.exits().next().unwrap().kind, ExitKind::Persistence);
    assert_eq!(verdict, AlwaysVerdict::None);
}

#[test]
fn always_host_rule_never_covers_open_network() {
    let engine = engine(vec![host("download.example.org", 443), host("localhost.example", 8080)]);
    let needs = |word: &str| Needs { hosts: vec![word.to_owned()], ..Needs::default() };
    let (decision, verdict) =
        judge(&engine, shell("make").with_needs(needs("download.example.org")));
    assert_eq!(
        decision.exits().next().unwrap().grants,
        [Grant::Host { host: "download.example.org".into(), port: 443 }]
    );
    assert_eq!(verdict, AlwaysVerdict::Covered { index: 0 });
    let (_, verdict) = judge(&engine, shell("make").with_needs(needs("Download.Example.org")));
    assert_eq!(verdict, AlwaysVerdict::Covered { index: 0 }, "a host matches without case");
    // Another port of the host is no match; the offer names it.
    let (_, verdict) =
        judge(&engine, shell("make").with_needs(needs("http://download.example.org/x")));
    assert_eq!(verdict, AlwaysVerdict::Offer(host("download.example.org", 80)));
    // A port other than 443 and 80 opens the whole network under the proxy too.
    let (decision, verdict) =
        judge(&engine, shell("make").with_needs(needs("download.example.org:8443")));
    assert_eq!(decision.exits().next().unwrap().grants, [Grant::OpenNetwork]);
    assert_eq!(verdict, AlwaysVerdict::None);
    // An IP address opens the whole network: no rule covers it.
    let (decision, verdict) = judge(&engine, shell("make").with_needs(needs("10.0.0.5")));
    assert_eq!(decision.exits().next().unwrap().grants, [Grant::OpenNetwork]);
    assert_eq!(verdict, AlwaysVerdict::None);
    // With no proxy every host opens the whole network.
    let none = engine.clone().with_support(AutoSupport::default());
    let (decision, verdict) = judge(&none, shell("make").with_needs(needs("download.example.org")));
    assert_eq!(decision.exits().next().unwrap().grants, [Grant::OpenNetwork]);
    assert_eq!(verdict, AlwaysVerdict::None);
}

#[test]
fn always_write_rule_never_covers_parent_above_under() {
    // The new dir `~/notes/sub` does not exist, so the "yes" binds `~/notes`, which lies
    // above the rule's dir.
    let file = "/home/u/notes/sub/a.txt";
    let requirements = shell("cp a.txt ~/notes/sub/a.txt").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/notes/sub", None), ("/home/u/notes", DIR)],
        &same(&[file, "/home/u/notes/sub", "/home/u/notes"]),
    ));
    let (decision, verdict) = judge(&engine(vec![write("~/notes/sub")]), requirements.clone());
    assert_eq!(
        decision.exits().next().unwrap().grants,
        [Grant::Write { path: "/home/u/notes".into() }]
    );
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/notes", "~/p/app")));
    assert_eq!(
        judge(&engine(vec![write("~/notes")]), requirements).1,
        AlwaysVerdict::Covered { index: 0 }
    );
}

#[test]
fn always_write_rule_never_covers_through_a_link() {
    // `~/n` links to `~/.config/hypr`: the real path of the bound dir is a floor.
    let file = "/home/u/n/a.conf";
    let requirements = shell("cp a.conf ~/n/a.conf").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/n", DIR)],
        &[(file, "/home/u/.config/hypr/a.conf"), ("/home/u/n", "/home/u/.config/hypr")],
    ));
    let (_, verdict) = judge(&engine(vec![write("~/n")]), requirements);
    assert_eq!(verdict, AlwaysVerdict::None);
    // `~/n` links to `~/docs`: a rule for `~/n` does not cover it, one for the real dir
    // does, and the offer names the real dir.
    let requirements = shell("cp a.txt ~/n/a.txt").with_write("/home/u/n/a.txt").with_facts(facts(
        &[("/home/u/n/a.txt", None), ("/home/u/n", DIR)],
        &[("/home/u/n/a.txt", "/home/u/docs/a.txt"), ("/home/u/n", "/home/u/docs")],
    ));
    let (_, verdict) = judge(&engine(vec![write("~/n")]), requirements.clone());
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/docs", "~/p/app")));
    assert_eq!(
        judge(&engine(vec![write("~/docs")]), requirements).1,
        AlwaysVerdict::Covered { index: 0 }
    );
}

#[test]
fn always_write_rule_never_covers_dir_above_missing_floor() {
    // `~/work/hooks` is a floor that does not exist yet (`sandbox.protect`): a bind of
    // `~/work` could make it, so no rule covers the dir bind.
    let engine = engine(vec![write("~/work")]);
    assert_eq!(marks(&engine), [None]);
    let file = "/home/u/work/a.txt";
    let dir_bind = shell("cp a.txt ~/work/a.txt")
        .with_write(file)
        .with_facts(facts(&[(file, None), ("/home/u/work", DIR)], &same(&[file, "/home/u/work"])));
    let (decision, verdict) = judge(&engine, dir_bind);
    assert_eq!(
        decision.exits().next().unwrap().bind,
        Some(WriteBind::Parent("/home/u/work".into()))
    );
    assert_eq!(verdict, AlwaysVerdict::None);
    // A bind of one new file stays eligible.
    let file_bind = shell("touch ~/work/b.txt").with_write("/home/u/work/b.txt").with_facts(facts(
        &[("/home/u/work/b.txt", None), ("/home/u/work", DIR)],
        &same(&["/home/u/work/b.txt", "/home/u/work"]),
    ));
    let (decision, verdict) = judge(&engine, file_bind);
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::MakeFile));
    assert_eq!(verdict, AlwaysVerdict::Covered { index: 0 });
}

#[test]
fn always_allow_under_dir_keeps_floor_binds() {
    // A rule above a project is allowed: the plan keeps the floor binds of the project
    // that exist. A dir bind with the project below it is never covered, because the
    // project's floors may be missing; a file bind next to it is.
    let engine = engine(vec![write("~/p/app/../x"), write("~/code")]);
    assert!(matches!(marks(&engine)[0], Some(AllowFloor::BadShape { .. })));
    let locations = machine().with_project(id(3), "/home/u/code/site").unwrap();
    let engine = Engine::with_defaults(locations).with_allow(vec![write("~/code")]);
    assert_eq!(marks(&engine), [None]);
    let dir = shell("cp a.txt ~/code/a.txt").with_write("/home/u/code/a.txt").with_facts(facts(
        &[("/home/u/code/a.txt", None), ("/home/u/code", DIR)],
        &same(&["/home/u/code/a.txt", "/home/u/code"]),
    ));
    assert_eq!(judge(&engine, dir).1, AlwaysVerdict::None);
    let file = shell("touch ~/code/b.txt").with_write("/home/u/code/b.txt").with_facts(facts(
        &[("/home/u/code/b.txt", None), ("/home/u/code", DIR)],
        &same(&["/home/u/code/b.txt", "/home/u/code"]),
    ));
    assert_eq!(judge(&engine, file).1, AlwaysVerdict::Covered { index: 0 });
}

#[test]
fn always_needs_real_path_facts() {
    let engine = engine(vec![write("~/notes"), socket("/run/user/1000/mpd/socket")]);
    let file = "/home/u/notes/a.txt";
    let no_fact = shell("cp a.txt ~/notes/a.txt")
        .with_write(file)
        .with_facts(facts(&[(file, None), ("/home/u/notes", DIR)], &[]));
    assert_eq!(judge(&engine, no_fact).1, AlwaysVerdict::None);
    // A fact of the target but none of the bound dir is not enough either.
    let target_only = shell("cp a.txt ~/notes/a.txt")
        .with_write(file)
        .with_facts(facts(&[(file, None), ("/home/u/notes", DIR)], &same(&[file])));
    assert_eq!(judge(&engine, target_only).1, AlwaysVerdict::None);
    let path = "/run/user/1000/mpd/socket";
    let needs = Needs { sockets: vec![path.to_owned()], ..Needs::default() };
    assert_eq!(judge(&engine, shell("mpc").with_needs(needs)).1, AlwaysVerdict::None);
    // The request names the paths: each written path and its parents below `~`, and
    // the sockets of `needs`.
    let request =
        crate::exits::fact_requests("echo x > ~/notes/sub/a.txt", Some(APP.as_ref()), &machine())
            .with_needs(
                Some(&Needs { sockets: vec![path.to_owned()], ..Needs::default() }),
                Some(APP.as_ref()),
                &machine(),
            )
            .with_writes(
                &[PathBuf::from("/home/u/docs/b.txt"), PathBuf::from("/etc/x")],
                &machine(),
            );
    let wanted: Vec<PathBuf> = [
        "/home/u/notes/sub/a.txt",
        "/home/u/notes/sub",
        "/home/u/notes",
        path,
        "/home/u/docs/b.txt",
        "/home/u/docs",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(request.real_paths, wanted);
}

#[test]
fn always_covers_only_when_every_exit_is_covered() {
    let engine = engine(vec![write("~/notes"), host("download.example.org", 443)]);
    // Two writes, one outside the rule's dir.
    let (a, b) = ("/home/u/notes/a.txt", "/home/u/docs/b.txt");
    let two = shell("cp a ~/notes/a.txt; cp b ~/docs/b.txt")
        .with_write(a)
        .with_write(b)
        .with_facts(facts(
            &[(a, None), ("/home/u/notes", DIR), (b, None), ("/home/u/docs", DIR)],
            &same(&[a, "/home/u/notes", b, "/home/u/docs"]),
        ));
    let (decision, verdict) = judge(&engine, two);
    assert_eq!(decision.exits().count(), 2);
    assert_eq!(verdict, AlwaysVerdict::None, "no one rule covers both, and no one rule may");
    // A write and a host: one rule each, but one rule must cover the whole call.
    let needs = Needs { hosts: vec!["download.example.org".to_owned()], ..Needs::default() };
    let (decision, verdict) = judge(&engine, notes_write().with_needs(needs));
    assert_eq!(decision.exits().count(), 2);
    assert_eq!(verdict, AlwaysVerdict::None);
    // Two writes below one dir: one rule covers both.
    let (a, b) = ("/home/u/notes/a.txt", "/home/u/notes/x/b.txt");
    let both = shell("cp a ~/notes/a.txt; cp b ~/notes/x/b.txt")
        .with_write(a)
        .with_write(b)
        .with_facts(facts(
            &[(a, None), ("/home/u/notes", DIR), (b, None), ("/home/u/notes/x", DIR)],
            &same(&[a, "/home/u/notes", b, "/home/u/notes/x"]),
        ));
    assert_eq!(judge(&engine, both.clone()).1, AlwaysVerdict::Covered { index: 0 });
    assert_eq!(
        judge(&self::engine(Vec::new()), both).1,
        AlwaysVerdict::Offer(write_in("~/notes", "~/p/app"))
    );
}

#[test]
fn always_offer_names_the_grant_path() {
    let engine = engine(Vec::new());
    // A file in a shared dir is bound alone, so the offer names the file.
    let file = "/home/u/todo.txt";
    let requirements = shell("echo x >> ~/todo.txt")
        .with_write(file)
        .with_facts(facts(&[(file, FILE)], &same(&[file])));
    let (decision, verdict) = judge(&engine, requirements);
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::TargetOnly));
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/todo.txt", "~/p/app")));
    // An existing dir is bound whole.
    let dir = "/home/u/notes/x";
    let requirements =
        shell("cp a.txt ~/notes/x").with_write(dir).with_facts(facts(&[(dir, DIR)], &same(&[dir])));
    let (decision, verdict) = judge(&engine, requirements);
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::Target));
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/notes/x", "~/p/app")));
    // With no project, the offer has none.
    let decision = decide_as(&engine, notes_write(), Origin::Shell);
    assert_eq!(engine.always(&decision, None), AlwaysVerdict::Offer(write("~/notes")));
    // A host.
    let needs = Needs { hosts: vec!["Mirror.Example.org".to_owned()], ..Needs::default() };
    assert_eq!(
        judge(&engine, shell("make").with_needs(needs)).1,
        AlwaysVerdict::Offer(host("mirror.example.org", 443))
    );
}

#[test]
fn always_write_rule_with_project_matches_only_that_project() {
    let engine = engine(vec![write_in("~/notes", "~/p/app")]);
    let decision = decide_as(&engine, notes_write(), Origin::Shell);
    assert_eq!(engine.always(&decision, Some(Path::new(APP))), AlwaysVerdict::Covered { index: 0 });
    assert_eq!(
        engine.always(&decision, Some(Path::new(LIB))),
        AlwaysVerdict::Offer(write_in("~/notes", "~/p/lib"))
    );
    assert_eq!(engine.always(&decision, None), AlwaysVerdict::Offer(write("~/notes")));
    // A project that is not registered matches nothing.
    let engine = self::engine(vec![write_in("~/notes", "~/p/gone")]);
    assert_eq!(marks(&engine), [None]);
    let decision = decide_as(&engine, notes_write(), Origin::Shell);
    assert_eq!(engine.always(&decision, Some(Path::new("/home/u/p/gone"))), AlwaysVerdict::None);
}

#[test]
fn always_covers_nothing_outside_auto_or_for_a_file_tool() {
    let engine = engine(vec![write("~/notes")]);
    let input = DecisionInput {
        requirements: notes_write(),
        scope: Scope::Project(app()),
        origin: Origin::Shell,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    };
    let decision = engine.decide(&input);
    assert_eq!(engine.always(&decision, Some(Path::new(APP))), AlwaysVerdict::None);
    // `write_file` outside the project is an exit, but a rule covers shell exits only.
    let file = "/home/u/notes/a.txt";
    let tool =
        Requirements::none().with_write(file).with_facts(facts(&[(file, None)], &same(&[file])));
    let (_, verdict) = judge(&engine, tool);
    assert_eq!(verdict, AlwaysVerdict::None);
}

/// A part of a path that the generated writes of [`always_offer_passes_the_load_check`]
/// use: plain names, floors, efr's dirs, a git dir, a synced folder, projects.
fn part() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "notes",
        "docs",
        "x",
        ".config",
        "systemd",
        "user",
        ".local",
        "share",
        "efr",
        "dbus-1",
        ".git",
        "hooks",
        "Dropbox",
        "p",
        "app",
        "lib",
        "work",
        ".ssh",
        "dotfiles",
        "hypr",
        ".zshrc",
        "autostart",
        ".cache",
        "code",
        "bin",
    ])
}

proptest! {
    #[test]
    fn always_offer_passes_the_load_check(
        parts in prop::collection::vec(part(), 1..5),
        exists in prop::collection::vec(any::<bool>(), 5),
        linked in any::<bool>(),
        port in prop::sample::select(vec![443_u16, 80, 8443]),
        name in prop::sample::select(vec!["download.example.org", "evil.example.org", "Mirror.Example.org"]),
    ) {
        let engine = engine(Vec::new());
        let file = PathBuf::from(HOME).join(parts.join("/")).join("a.txt");
        let mut targets: Vec<(PathBuf, Option<TargetKind>)> = Vec::new();
        let mut real: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (dir, there) in file.ancestors().zip(exists.iter().chain(std::iter::repeat(&true))) {
            if !dir.starts_with(HOME) || dir == Path::new(HOME) {
                break;
            }
            let kind = if *there { if dir == file { FILE } else { DIR } } else { None };
            targets.push((dir.to_path_buf(), kind));
            let resolved = if linked {
                PathBuf::from("/home/u/.config/hypr").join(dir.strip_prefix(HOME).unwrap())
            } else {
                dir.to_path_buf()
            };
            real.push((dir.to_path_buf(), resolved));
        }
        let facts = CallFacts { targets, real_paths: real, ..CallFacts::default() };
        let line = format!("echo x > {}", file.display());
        let requirements = shell(&line).with_write(&file).with_facts(facts);
        let decision = decide_as(&engine, requirements, Origin::Shell);
        let verdict = engine.always(&decision, Some(Path::new(APP)));
        assert_sound(&engine, &decision, &verdict);
        if let AlwaysVerdict::Offer(rule) = &verdict {
            prop_assert_eq!(engine.check_allow(rule), Ok(()));
            let again = engine.clone().with_allow(vec![rule.clone()]);
            prop_assert_eq!(again.always(&decision, Some(Path::new(APP))), AlwaysVerdict::Covered { index: 0 });
        }
        let needs = Needs { hosts: vec![format!("{name}:{port}")], ..Needs::default() };
        let decision = decide_as(&engine, shell("make").with_needs(needs), Origin::Shell);
        let verdict = engine.always(&decision, Some(Path::new(APP)));
        assert_sound(&engine, &decision, &verdict);
        if let AlwaysVerdict::Offer(rule) = &verdict {
            prop_assert_eq!(engine.check_allow(rule), Ok(()));
        }
    }
}

#[test]
fn always_write_rule_never_covers_a_git_dir() {
    // `~/notes` is a repository that is no project. A rule of `~/notes` loads, but a
    // write into its git dir could plant a hook or a config key that runs code, and the
    // surface guard looks only for `.git` at and below a grant.
    let engine = engine(vec![write("~/notes")]);
    assert_eq!(marks(&engine), [None]);
    let hook = "/home/u/notes/.git/hooks/pre-commit";
    let hooks = "/home/u/notes/.git/hooks";
    let copy = shell("cp hook ~/notes/.git/hooks/pre-commit")
        .with_write(hook)
        .with_facts(facts(&[(hook, None), (hooks, DIR)], &same(&[hook, hooks])));
    let (decision, verdict) = judge(&engine, copy.clone());
    let need = decision.exits().next().unwrap();
    assert_eq!(need.bind, Some(WriteBind::Parent(hooks.into())));
    assert_eq!(need.approver, Approver::Classifier);
    assert_eq!(verdict, AlwaysVerdict::None);
    assert_eq!(judge(&self::engine(Vec::new()), copy).1, AlwaysVerdict::None);
    // The config of the same repository.
    let config = "/home/u/notes/.git/config";
    let append = shell("echo x >> ~/notes/.git/config").with_write(config).with_facts(facts(
        &[(config, FILE), ("/home/u/notes/.git", DIR)],
        &same(&[config, "/home/u/notes/.git"]),
    ));
    let (decision, verdict) = judge(&engine, append);
    assert!(decision.exits().next().is_some());
    assert_eq!(verdict, AlwaysVerdict::None);
    // A link that leads into the git dir.
    let file = "/home/u/notes/h/pre-commit";
    let linked = shell("cp hook ~/notes/h/pre-commit").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/notes/h", DIR)],
        &[(file, hook), ("/home/u/notes/h", hooks)],
    ));
    assert_eq!(judge(&engine, linked).1, AlwaysVerdict::None);
}

#[test]
fn always_write_rule_never_covers_a_floor_through_a_link() {
    // `~/work/h` links to the floor `~/work/hooks`: the new file lies in the floor.
    let engine = engine(vec![write("~/work")]);
    assert_eq!(marks(&engine), [None]);
    let (file, real) = ("/home/u/work/h/b.txt", "/home/u/work/hooks/b.txt");
    let requirements = shell("touch ~/work/h/b.txt").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/work/h", DIR)],
        &[(file, real), ("/home/u/work/h", "/home/u/work/hooks")],
    ));
    let (decision, verdict) = judge(&engine, requirements);
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::MakeFile));
    assert_eq!(verdict, AlwaysVerdict::None);
}

#[test]
fn always_offers_no_rule_for_a_new_file() {
    // A rule of a new file would be dead: once the file exists, a write of it binds its
    // dir, and the rule never covers the dir.
    let file = "/home/u/notes/b.txt";
    let new = shell("touch ~/notes/b.txt").with_write(file).with_facts(facts(
        &[(file, None), ("/home/u/notes", DIR)],
        &same(&[file, "/home/u/notes"]),
    ));
    let (decision, verdict) = judge(&engine(Vec::new()), new);
    assert_eq!(decision.exits().next().unwrap().bind, Some(WriteBind::MakeFile));
    assert_eq!(verdict, AlwaysVerdict::None);
    let again = shell("touch ~/notes/b.txt").with_write(file).with_facts(facts(
        &[(file, FILE), ("/home/u/notes", DIR)],
        &same(&[file, "/home/u/notes"]),
    ));
    let engine = engine(vec![write("~/notes/b.txt")]);
    assert_eq!(marks(&engine), [None]);
    let (decision, verdict) = judge(&engine, again);
    assert_eq!(
        decision.exits().next().unwrap().bind,
        Some(WriteBind::Parent("/home/u/notes".into()))
    );
    assert_eq!(verdict, AlwaysVerdict::Offer(write_in("~/notes", "~/p/app")));
    // A rule of the dir still covers the new file.
    let new = shell("touch ~/notes/c.txt").with_write("/home/u/notes/c.txt").with_facts(facts(
        &[("/home/u/notes/c.txt", None), ("/home/u/notes", DIR)],
        &same(&["/home/u/notes/c.txt", "/home/u/notes"]),
    ));
    assert_eq!(
        judge(&self::engine(vec![write("~/notes")]), new).1,
        AlwaysVerdict::Covered { index: 0 }
    );
}
