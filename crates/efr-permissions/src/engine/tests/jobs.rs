//! Background jobs (phase C): the exits of a job are those of the same line in the
//! foreground, except that no delete of it counts as covered; the kinds that no job may
//! hold are refused in every mode; outside `auto` and full mode each job asks the user.

use efr_protocol::{BusKind, ExitKind, Grant, Mode, Needs, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::coverage::{coverable, undo_engine};
use super::{app, input};
use crate::{
    Action, AutoSupport, Cause, Decision, DecisionInput, Effect, Engine, Policy, Requirements,
    Resource, Rule, Subject,
};

const APP: &str = "/home/u/p/app";

fn engine() -> Engine {
    Engine::with_defaults(auto_locations())
}

/// A shell call of `line` in the project root that starts a background job.
fn job(line: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(APP).with_background()
}

fn decide_in(engine: &Engine, requirements: Requirements, mode: Mode, origin: Origin) -> Decision {
    let input = DecisionInput { mode, ..input(requirements, Scope::Project(app()), origin) };
    engine.decide(&input)
}

fn decide(requirements: Requirements) -> Decision {
    decide_in(&engine(), requirements, Mode::Auto, Origin::Shell)
}

/// The kinds that the refusals of `decision` name, in order.
fn refused(decision: &Decision) -> Vec<ExitKind> {
    decision
        .reasons()
        .iter()
        .filter_map(|reason| match (&reason.subject, &reason.cause) {
            (Subject::JobPart { kind, .. }, Cause::NotAsJob) => Some(*kind),
            _ => None,
        })
        .collect()
}

#[test]
fn contained_job_runs_with_no_exit() {
    let decision = decide(job("npm run dev"));
    assert_eq!(decision.effect(), Effect::Contain);
    assert_eq!(decision.exits().count(), 0);
}

#[test]
fn job_destructive_not_routine() {
    // A covered delete runs with no question in the foreground, but a job answers at
    // once and may still write when the user undoes its turn.
    let engine = undo_engine();
    for requirements in coverable() {
        let line = requirements.command.clone();
        let foreground = decide_in(&engine, requirements.clone(), Mode::Auto, Origin::Shell);
        assert_eq!(foreground.exits().count(), 0, "{line:?}");
        let background =
            decide_in(&engine, requirements.with_background(), Mode::Auto, Origin::Shell);
        let kinds: Vec<ExitKind> = background.exits().map(|need| need.kind).collect();
        assert!(kinds.contains(&ExitKind::Destructive), "{line:?}: {kinds:?}");
        assert_eq!(background.effect(), Effect::Ask, "{line:?}");
        assert_eq!(refused(&background), [], "{line:?}: a delete may run as a job");
    }
}

#[rstest]
#[case::privilege(job("docker run --rm alpine"), ExitKind::Privilege)]
#[case::pkexec(job("pkexec true"), ExitKind::Privilege)]
fn privilege_job_refused(#[case] requirements: Requirements, #[case] kind: ExitKind) {
    let decision = decide(requirements);
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(refused(&decision), [kind]);
}

#[test]
fn masked_read_job_refused() {
    let decision = decide(job("cat .env").with_read("/home/u/p/app/.env"));
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(refused(&decision), [ExitKind::MaskedRead]);
}

#[rstest]
#[case::zshrc("echo 'alias ll=ls' >> ~/.zshrc", "/home/u/.zshrc")]
#[case::crontab("crontab -e", "")]
fn persistence_job_refused(#[case] line: &str, #[case] written: &str) {
    let mut requirements = job(line);
    if !written.is_empty() {
        requirements = requirements.with_write(written);
    }
    let decision = decide(requirements);
    assert_eq!(decision.effect(), Effect::Deny);
    assert!(refused(&decision).contains(&ExitKind::Persistence), "{:?}", refused(&decision));
}

#[test]
fn desktop_type_job_refused() {
    let decision = decide(job("xdotool type hello"));
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(refused(&decision), [ExitKind::DesktopIpc]);
    // A read of the desktop's state is no job refusal.
    let read = decide(job("hyprctl monitors"));
    assert_eq!(refused(&read), []);
}

#[rstest]
#[case::predicted(job("hostnamectl"))]
#[case::needs(job("my-tool").with_needs(Needs { bus: Some(BusKind::Session), ..Needs::default() }))]
fn bus_job_refused(#[case] requirements: Requirements) {
    let decision = decide(requirements);
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(refused(&decision), [ExitKind::Bus]);
}

#[rstest]
#[case::podman("/run/user/1000/podman/podman.sock")]
#[case::containerd("/run/containerd/containerd.sock")]
#[case::systemd("/run/user/1000/systemd/private")]
fn container_socket_job_refused(#[case] socket: &str) {
    let needs = Needs { sockets: vec![socket.to_owned()], ..Needs::default() };
    let decision = decide(job("my-tool").with_needs(needs.clone()));
    assert_eq!(decision.effect(), Effect::Deny, "{socket}");
    assert_eq!(refused(&decision).len(), 1, "{socket}: {:?}", refused(&decision));
    // The same socket in the foreground asks, as before.
    let foreground = decide(
        Requirements::none().with_command("my-tool").with_command_dir(APP).with_needs(needs),
    );
    assert_eq!(foreground.effect(), Effect::Ask, "{socket}");
}

#[test]
fn the_refusal_says_to_run_that_part_in_the_foreground() {
    let decision = decide(job("cargo build && xdotool type done"));
    let reason = decision.deciding().next().unwrap();
    let text = reason.for_model().to_string();
    assert_eq!(
        text,
        "run \"xdotool type done\" as a job (desktop_ipc): deny, because this cannot run as a \
         job; run that part in the foreground, and the rest as a job"
    );
}

#[rstest]
#[case::cautious(Mode::Cautious)]
#[case::manual(Mode::Manual)]
fn a_job_outside_auto_asks_the_user_whatever_the_rules_say(#[case] mode: Mode) {
    let allow_all =
        Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap();
    for engine in [engine(), Engine::with_rules(auto_locations(), allow_all)] {
        let decision = decide_in(&engine, job("ls -la"), mode, Origin::Shell);
        assert_eq!(decision.effect(), Effect::Ask, "{mode}");
        let reason = decision.deciding().find(|reason| reason.subject == Subject::Job).unwrap();
        assert_eq!(reason.cause, Cause::JobAsks, "{mode}");
        assert_eq!(decision.exits().count(), 0, "{mode}: outside auto a job has no exits");
    }
    // The same line in the foreground runs at once in cautious.
    let foreground = Requirements::none().with_command("ls -la").with_command_dir(APP);
    assert_eq!(
        decide_in(&engine(), foreground, Mode::Cautious, Origin::Shell).effect(),
        Effect::Allow
    );
}

#[test]
fn an_auto_turn_from_a_phone_asks_as_cautious() {
    let decision = decide_in(&engine(), job("ls"), Mode::Auto, Origin::Phone);
    assert_eq!(decision.effect(), Effect::Ask);
    assert!(decision.reasons().iter().any(|reason| reason.cause == Cause::JobAsks));
}

#[rstest]
#[case::cautious(Mode::Cautious)]
#[case::manual(Mode::Manual)]
fn a_refused_kind_stays_refused_outside_auto(#[case] mode: Mode) {
    for (line, kind) in
        [("docker run --rm alpine", ExitKind::Privilege), ("xdotool type x", ExitKind::DesktopIpc)]
    {
        let decision = decide_in(&engine(), job(line), mode, Origin::Shell);
        assert_eq!(decision.effect(), Effect::Deny, "{mode}: {line}");
        assert_eq!(refused(&decision), [kind], "{mode}: {line}");
    }
}

#[rstest]
#[case::manual(Mode::Manual)]
#[case::cautious(Mode::Cautious)]
#[case::auto(Mode::Auto)]
#[case::full(Mode::Full)]
fn jobs_tool_needs_no_question(#[case] mode: Mode) {
    // The `jobs` and `job_stop` tools declare nothing: a local origin runs them in every
    // mode with no question. A phone turn gets the usual question of a remote origin.
    for origin in [Origin::Shell, Origin::Cli, Origin::Proxy] {
        let decision = decide_in(&engine(), Requirements::none(), mode, origin);
        assert_eq!(decision.effect(), Effect::Allow, "{mode} from {origin:?}");
    }
}

#[test]
fn nmcli_job_is_refused() {
    // `nmcli` asks NetworkManager on the raw system bus, which no job may hold, also in
    // a read form that the classifier could judge in the foreground.
    for line in ["nmcli device status", "nmcli radio wifi off", "iwctl station wlan0 show"] {
        let decision = decide(job(line));
        assert_eq!(decision.effect(), Effect::Deny, "{line:?}");
        assert_eq!(refused(&decision), [ExitKind::Bus], "{line:?}");
    }
}

/// The machine of [`engine`] with the filtered bus proxy (phase E).
fn bus_proxy_engine() -> Engine {
    engine().with_support(AutoSupport { bus_proxy: true, ..AutoSupport::default() })
}

#[rstest]
#[case::failed("systemctl --failed", BusKind::System)]
#[case::user_failed("systemctl --user --failed", BusKind::Session)]
#[case::watch("watch -n 60 systemctl list-units --state=failed", BusKind::System)]
#[case::sessions("loginctl list-sessions", BusKind::System)]
fn job_with_bus_read_form_gets_the_view(#[case] line: &str, #[case] bus: BusKind) {
    // A view is no exit, so `job_refusal` never sees it: the job reads its own filtered
    // bus for its whole life (Q63, DE12).
    let decision = decide_in(&bus_proxy_engine(), job(line), Mode::Auto, Origin::Shell);
    assert_eq!(decision.effect(), Effect::Contain, "{line:?}");
    assert_eq!(refused(&decision), [], "{line:?}");
    let views: Vec<&Grant> = decision.view_grants().collect();
    assert_eq!(views, [&Grant::BusFilter { bus }], "{line:?}");
    // Without the proxy the read form is a `bus` exit, which no job may hold.
    let decision = decide(job(line));
    assert_eq!(decision.effect(), Effect::Deny, "{line:?}");
    assert_eq!(refused(&decision), [ExitKind::Bus], "{line:?}");
}

#[rstest]
#[case::user_restart("systemctl --user restart x")]
#[case::nmcli("nmcli device status")]
#[case::notify("notify-send done")]
#[case::remote("systemctl -H box status x")]
#[case::beside_a_read_form("systemctl --failed; busctl list")]
fn job_with_bus_change_form_stays_refused(#[case] line: &str) {
    let engine = bus_proxy_engine();
    let decision = decide_in(&engine, job(line), Mode::Auto, Origin::Shell);
    assert_eq!(decision.effect(), Effect::Deny, "{line:?}");
    assert_eq!(refused(&decision), [ExitKind::Bus], "{line:?}");
    // The model's own need of the whole bus stays refused for a job too.
    let asked = Needs { bus: Some(BusKind::System), ..Needs::default() };
    let decision =
        decide_in(&engine, job("hostnamectl").with_needs(asked), Mode::Auto, Origin::Shell);
    assert_eq!(refused(&decision), [ExitKind::Bus], "{line:?}");
}

#[test]
fn full_job_starts_without_a_question() {
    // DF21: a full job starts as an `auto` job does, with no `JobAsks`, and with nothing
    // contained: there is no sandbox.
    for line in ["npm run dev", "cargo watch -x test", "python3 server.py"] {
        let decision = decide_in(&engine(), job(line), Mode::Full, Origin::Shell);
        assert_eq!(decision.effect(), Effect::Allow, "{line:?}");
        assert!(
            !decision.reasons().iter().any(|reason| reason.cause == Cause::JobAsks),
            "{line:?}"
        );
    }
}

#[rstest]
#[case::privilege(job("docker run --rm alpine"), ExitKind::Privilege)]
#[case::pkexec(job("pkexec true"), ExitKind::Privilege)]
#[case::masked_read(job("cat .env").with_read("/home/u/p/app/.env"), ExitKind::MaskedRead)]
#[case::persistence(job("crontab -e"), ExitKind::Persistence)]
#[case::zshrc(
    job("echo 'alias ll=ls' >> ~/.zshrc").with_write("/home/u/.zshrc"),
    ExitKind::Persistence
)]
#[case::desktop(job("xdotool type hello"), ExitKind::DesktopIpc)]
#[case::bus(job("hostnamectl"), ExitKind::Bus)]
fn full_job_refusals_hold(#[case] requirements: Requirements, #[case] kind: ExitKind) {
    for engine in [engine(), bus_proxy_engine()] {
        let decision = decide_in(&engine, requirements.clone(), Mode::Full, Origin::Shell);
        assert_eq!(decision.effect(), Effect::Deny, "{kind}");
        assert!(refused(&decision).contains(&kind), "{kind}: {:?}", refused(&decision));
    }
}

#[rstest]
#[case::failed("systemctl --failed")]
#[case::user_failed("systemctl --user --failed")]
#[case::sessions("loginctl list-sessions")]
fn full_job_bus_query_refused(#[case] line: &str) {
    // B19, OQF27: with no sandbox efr cannot hold a job's bus to a filter, so a read
    // form that an `auto` job runs through its filtered bus is a `bus` exit, which no
    // job may hold, also on a machine with the bus proxy.
    let auto = decide_in(&bus_proxy_engine(), job(line), Mode::Auto, Origin::Shell);
    assert_eq!(auto.effect(), Effect::Contain, "{line:?}");
    let full = decide_in(&bus_proxy_engine(), job(line), Mode::Full, Origin::Shell);
    assert_eq!(full.effect(), Effect::Deny, "{line:?}");
    assert_eq!(refused(&full), [ExitKind::Bus], "{line:?}");
    assert_eq!(full.view_grants().count(), 0, "{line:?}");
    // In the foreground it runs and is recorded.
    let foreground = Requirements::none().with_command(line).with_command_dir(APP);
    let full = decide_in(&bus_proxy_engine(), foreground, Mode::Full, Origin::Shell);
    assert_eq!(full.effect(), Effect::Allow, "{line:?}");
    assert!(full.exits().any(|need| need.kind == ExitKind::Bus), "{line:?}");
}
