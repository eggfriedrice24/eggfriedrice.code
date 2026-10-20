mod naming;
mod review;

use std::path::{Path, PathBuf};

use efr_permissions::{
    Approver, AutoSupport, CallFacts, ConversationPolicy, Decision, DecisionInput, Egress, Engine,
    Locations, Requirements, TargetKind,
};
use efr_protocol::{
    BusKind, DesktopKind, Event, EventEnvelope, ExitInfo, ExitKind, ExitRecord, ExitSource, Grant,
    Launch, Mode, Needs, Origin, PathClassName, ProjectId, SandboxSummary, Scope, Seq,
    SurfaceChange, TurnId, Verdict,
};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Clock;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{
    Action, TurnExits, bounded, change_names, floor_kinds, grant, info, kinds, quarantined, route,
    source, tilde, user_messages,
};

const HOME: &str = "/home/u";
const APP: &str = "/home/u/p/app";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-04-fix-0a1b2c3d";

fn app() -> ProjectId {
    "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap()
}

fn engine() -> Engine {
    let locations = Locations::new(HOME)
        .unwrap()
        .with_project(app(), APP)
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap()
        .with_synced_root("/home/u/Dropbox")
        .unwrap()
        .with_envelope_root("/tmp")
        .unwrap();
    Engine::with_defaults(locations)
}

/// The record of `action`, as [`super::record`] makes it. Every record of these tests
/// also passes the check of the wave 2 integration: `on_allow_list` holds only under
/// efr's proxy, and only for a host whose exits have `Grant::Host` grants only.
fn record(action: Action<'_>, messages: &[String], turn: &TurnExits) -> ExitRecord {
    let made = super::record(action, messages, turn);
    for fact in made.facts.hosts.iter().filter(|fact| fact.on_allow_list) {
        assert_eq!(action.engine.support().egress, Egress::Proxy, "{fact:?}");
        for need in action.decision.exits().filter(|need| need.hosts.contains(&fact.host)) {
            assert!(
                need.grants.iter().all(|grant| matches!(grant, Grant::Host { .. })),
                "{fact:?} in {need:?}"
            );
        }
    }
    made
}

/// A shell call of `line` in the project root.
fn shell(line: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(APP)
}

fn decide(engine: &Engine, requirements: Requirements) -> Decision {
    engine.decide(&DecisionInput {
        requirements,
        scope: Scope::Project(app()),
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    })
}

fn launch_of(line: &str) -> Launch {
    grant(&decide(&engine(), shell(line)))
}

/// The question about the exits of `decision`, with no user message that names one.
fn question(decision: &Decision, requirements: &Requirements, launch: &Launch) -> ExitInfo {
    let approver = route(decision, &[], None).unwrap_or(Approver::User);
    info(decision, requirements, launch, Path::new(HOME), approver)
}

#[test]
fn a_line_without_exits_runs_in_the_plain_sandbox() {
    assert_eq!(launch_of("cargo test"), Launch::contained());
    assert_eq!(launch_of("rm -rf target"), Launch::contained());
}

#[test]
fn a_destructive_exit_keeps_the_plain_sandbox() {
    let decision = decide(&engine(), shell("git reset --hard"));
    assert_eq!(kinds(&decision), vec![ExitKind::Destructive]);
    assert_eq!(grant(&decision), Launch::contained());
}

#[test]
fn a_network_exit_opens_the_network_for_the_call() {
    let decision = decide(&engine(), shell("curl -LO https://example.com/tool.tar.gz"));
    assert_eq!(kinds(&decision), vec![ExitKind::Host]);
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![Grant::OpenNetwork] });
}

#[test]
fn a_write_exit_binds_one_path() {
    let decision = decide(&engine(), shell("echo note >> /home/u/notes/today.txt"));
    assert_eq!(kinds(&decision), vec![ExitKind::Write]);
    // NOTE: no fact says whether the file exists, so it counts as an existing file, and
    // its parent, not a shared directory, is bound.
    assert_eq!(
        grant(&decision),
        Launch::Contained { grants: vec![Grant::Write { path: "/home/u/notes".into() }] }
    );
}

#[test]
fn privilege_persistence_and_upload_run_in_the_exit_child() {
    for line in ["sudo pacman -Syu", "echo 'alias k=kubectl' >> ~/.zshrc", "git push"] {
        assert_eq!(launch_of(line), Launch::Unsandboxed, "{line}");
    }
}

#[test]
fn a_mix_of_contained_exits_gets_every_grant_once() {
    let needs = Needs {
        bus: Some(BusKind::System),
        hosts: vec!["example.com".to_owned()],
        ..Needs::default()
    };
    let decision = decide(&engine(), shell("curl https://example.com && curl x").with_needs(needs));
    let Launch::Contained { grants } = grant(&decision) else {
        panic!("contained: {decision:?}");
    };
    assert!(grants.contains(&Grant::OpenNetwork), "{grants:?}");
    assert!(grants.contains(&Grant::Bus { bus: BusKind::System }), "{grants:?}");
    assert_eq!(grants.iter().filter(|grant| **grant == Grant::OpenNetwork).count(), 1);
    assert_eq!(source(&decision), ExitSource::Needs);
}

#[test]
fn grant_keeps_desktop_grant_in_contained_launch() {
    let hyprland = Grant::Desktop { desktop: DesktopKind::Hyprland };
    assert_eq!(launch_of("hyprctl monitors"), Launch::Contained { grants: vec![hyprland.clone()] });
    assert_eq!(
        launch_of("hyprctl dispatch exec kitty"),
        Launch::Contained { grants: vec![hyprland.clone()] },
        "a form that only the user approves still runs in the sandbox with the socket"
    );
    let Launch::Contained { grants } = launch_of("hyprctl monitors && wlr-randr") else {
        panic!("contained");
    };
    assert_eq!(grants, [hyprland, Grant::Desktop { desktop: DesktopKind::Wayland }]);
    assert_eq!(launch_of("ydotool type x"), Launch::Contained { grants: vec![] });
}

/// The launch of `decision` when the engine also gave the line the network view.
fn with_net_view(decision: &Decision) -> Launch {
    super::launch(decision.exits(), [Grant::NetView].iter())
}

/// The decision of a routine `make` with the exits that `needs` declares.
fn needs_decision(engine: &Engine, needs: Needs) -> Decision {
    decide(engine, shell("make").with_needs(needs))
}

#[test]
fn grant_unions_view_and_exit_grants() {
    // NOTE: the engine of this phase gives no view reason (the line rule comes later),
    // so the views come in beside the exits of a line that the engine decided.
    assert_eq!(
        with_net_view(&decide(&engine(), shell("cat /proc/net/dev"))),
        Launch::Contained { grants: vec![Grant::NetView] },
        "a view alone runs routine with its grant"
    );
    assert_eq!(
        super::launch(std::iter::empty(), [Grant::NetView, Grant::NetView].iter()),
        Launch::Contained { grants: vec![Grant::NetView] },
        "each grant once"
    );
    assert_eq!(
        super::launch(std::iter::empty(), [Grant::Unknown].iter()),
        Launch::contained(),
        "a view that this build does not know never joins"
    );
    let decision = decide(&engine(), shell("cat /proc/net/dev"));
    assert_eq!(decision.view_grants().count(), 0);
    assert_eq!(grant(&decision), Launch::contained(), "no view reason, no view grant");
}

#[test]
fn grant_keeps_net_view_with_write_grant() {
    let decision = decide(&engine(), shell("cat /proc/net/dev >> /home/u/notes/dev.txt"));
    assert_eq!(kinds(&decision), [ExitKind::Write]);
    assert_eq!(
        with_net_view(&decision),
        Launch::Contained {
            grants: vec![Grant::Write { path: "/home/u/notes".into() }, Grant::NetView]
        }
    );
}

#[test]
fn grant_keeps_net_view_with_unmask_grant() {
    let decision = needs_decision(
        &engine(),
        Needs { unmask: vec!["~/.zsh_history".to_owned()], ..Needs::default() },
    );
    let unmask = Grant::Unmask { path: "/home/u/.zsh_history".into() };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![unmask.clone()] });
    assert_eq!(
        with_net_view(&decision),
        Launch::Contained { grants: vec![unmask, Grant::NetView] }
    );
}

#[test]
fn grant_drops_net_view_with_open_network() {
    let decision = decide(&engine(), shell("cat /proc/net/dev && curl -LO https://example.com/x"));
    assert_eq!(kinds(&decision), [ExitKind::Host]);
    assert_eq!(with_net_view(&decision), Launch::Contained { grants: vec![Grant::OpenNetwork] });
}

#[test]
fn grant_drops_net_view_with_host_grant() {
    let proxy =
        engine().with_support(AutoSupport { egress: Egress::Proxy, ..AutoSupport::default() });
    let decision =
        needs_decision(&proxy, Needs { hosts: vec!["crates.io".to_owned()], ..Needs::default() });
    let host = Grant::Host { host: "crates.io".to_owned(), port: 443 };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![host.clone()] });
    assert_eq!(with_net_view(&decision), Launch::Contained { grants: vec![host] });
}

#[test]
fn grant_drops_net_view_with_device_grant() {
    let decision = decide(&engine(), shell("nvme smart-log /dev/nvme0n1"));
    let device = Grant::Device { path: "/dev/nvme0n1".into() };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![device.clone()] });
    assert_eq!(
        with_net_view(&decision),
        Launch::Contained { grants: vec![device] },
        "a device such as /dev/net/tun could send from the host's namespace"
    );
}

#[test]
fn grant_drops_net_view_with_socket_grant() {
    let needs = Needs { sockets: vec!["/run/user/1000/app.sock".to_owned()], ..Needs::default() };
    let decision = needs_decision(&engine(), needs);
    let socket = Grant::Socket { path: "/run/user/1000/app.sock".into() };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![socket.clone()] });
    assert_eq!(with_net_view(&decision), Launch::Contained { grants: vec![socket] });
}

#[test]
fn grant_drops_net_view_with_bus_grant() {
    let decision =
        needs_decision(&engine(), Needs { bus: Some(BusKind::System), ..Needs::default() });
    let bus = Grant::Bus { bus: BusKind::System };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![bus.clone()] });
    assert_eq!(with_net_view(&decision), Launch::Contained { grants: vec![bus] });
    let desktop = decide(&engine(), shell("hyprctl monitors"));
    let hyprland = Grant::Desktop { desktop: DesktopKind::Hyprland };
    assert_eq!(with_net_view(&desktop), Launch::Contained { grants: vec![hyprland] });
}

#[test]
fn grant_unsandboxed_wins_over_net_view() {
    for line in ["sudo ss -tlnp", "cat /proc/net/dev; git push"] {
        let decision = decide(&engine(), shell(line));
        assert_eq!(with_net_view(&decision), Launch::Unsandboxed, "{line}");
    }
}

/// The engine of [`engine`] on a machine with the filtered bus proxy (phase E), and
/// efr's network proxy when `proxy` is true.
fn bus_proxy_engine(proxy: bool) -> Engine {
    let egress = if proxy { Egress::Proxy } else { Egress::None };
    engine().with_support(AutoSupport { bus_proxy: true, egress, ..AutoSupport::default() })
}

#[test]
fn bus_view_fits_beside_write_and_host_but_not_beside_real_bus() {
    let system = Grant::BusFilter { bus: BusKind::System };
    let session = Grant::BusFilter { bus: BusKind::Session };
    // Alone, the read form runs routine with its own filtered bus.
    let decision = decide(&bus_proxy_engine(false), shell("systemctl --failed"));
    assert_eq!(kinds(&decision), []);
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![system.clone()] });
    // Beside a write grant.
    let decision =
        decide(&bus_proxy_engine(false), shell("systemctl --failed > /home/u/notes/failed.txt"));
    assert_eq!(kinds(&decision), [ExitKind::Write]);
    let write = Grant::Write { path: "/home/u/notes".into() };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![write, system.clone()] });
    // Beside a host grant of efr's proxy.
    let needs = Needs { hosts: vec!["example.org".to_owned()], ..Needs::default() };
    let decision = decide(&bus_proxy_engine(true), shell("hostnamectl").with_needs(needs));
    let host = Grant::Host { host: "example.org".to_owned(), port: 443 };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![host, system.clone()] });
    // Beside the real bus of the other bus it stays; beside the real bus of its own bus
    // it drops, and the approved exit serves the reads too.
    let decision = decide(&bus_proxy_engine(false), shell("systemctl --failed; notify-send done"));
    let real_session = Grant::Bus { bus: BusKind::Session };
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![real_session, system.clone()] });
    let decision =
        decide(&bus_proxy_engine(false), shell("systemctl --user --failed; notify-send done"));
    assert_eq!(
        grant(&decision),
        Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::Session }] }
    );
    let asked = Needs { bus: Some(BusKind::System), ..Needs::default() };
    let decision = decide(&bus_proxy_engine(false), shell("hostnamectl").with_needs(asked));
    assert_eq!(
        grant(&decision),
        Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::System }] }
    );
    // Both buses, each with its own view.
    let decision =
        decide(&bus_proxy_engine(false), shell("systemctl --failed; systemctl --user --failed"));
    assert_eq!(grant(&decision), Launch::Contained { grants: vec![system, session] });
    // A line that runs outside the sandbox gets no view at all.
    let decision = decide(&bus_proxy_engine(false), shell("sudo systemctl status nginx"));
    assert_eq!(grant(&decision), Launch::Unsandboxed);
}

#[test]
fn netview_and_bus_filter_never_join_one_launch() {
    let system = Grant::BusFilter { bus: BusKind::System };
    // The engine gives the network view first, so the bus view drops after it.
    assert_eq!(
        super::launch(std::iter::empty(), [Grant::NetView, system.clone()].iter()),
        Launch::Contained { grants: vec![Grant::NetView] }
    );
    // In the other order the network view drops: the two never join.
    assert_eq!(
        super::launch(std::iter::empty(), [system.clone(), Grant::NetView].iter()),
        Launch::Contained { grants: vec![system] }
    );
    // A line of the network view rule holds no systemd tool, and a line with one gets no
    // network view.
    let netview = AutoSupport { bus_proxy: true, netview: true, ..AutoSupport::default() };
    let engine = engine().with_support(netview);
    let decision = decide(&engine, shell("ss -tln; systemctl --failed"));
    let views: Vec<&Grant> = decision.view_grants().collect();
    assert_eq!(views, [&Grant::BusFilter { bus: BusKind::System }]);
    assert_eq!(kinds(&decision), [ExitKind::HostView]);
    let decision = decide(&engine, shell("ss -tln | grep 8080"));
    let views: Vec<&Grant> = decision.view_grants().collect();
    assert_eq!(views, [&Grant::NetView]);
}

/// The record of a call whose line is `line`, with no facts and no messages.
fn record_of(line: &str) -> ExitRecord {
    let engine = engine();
    let requirements = shell(line);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };
    record(action, &[], &TurnExits::default())
}

#[test]
fn a_desktop_read_form_is_recorded_as_a_read() {
    let read = record_of("hyprctl -j monitors");
    assert!(read.facts.desktop_read);
    assert_eq!(read.action.grants, [Grant::Desktop { desktop: DesktopKind::Hyprland }]);
    for line in ["hyprctl dispatch exec kitty", "hyprctl monitors; hyprctl keyword x y", "ls"] {
        assert!(!record_of(line).facts.desktop_read, "{line}");
    }
}

#[test]
fn a_floor_opens_nothing_and_is_named() {
    let engine = engine();
    let decision =
        decide(&engine, shell("cat ~/.ssh/id_ed25519").with_read("/home/u/.ssh/id_ed25519"));
    assert_eq!(floor_kinds(&decision), vec![ExitKind::Secret]);
    assert_eq!(grant(&decision), Launch::contained());
    let config = decide(&engine, shell("true").with_write("/home/u/.config/efr/config.toml"));
    assert_eq!(floor_kinds(&config), vec![ExitKind::Config]);
    assert!(floor_kinds(&decide(&engine, shell("cargo test"))).is_empty());
}

#[test]
fn the_question_says_what_efr_knows_and_what_the_model_says() {
    let engine = engine();
    let needs =
        Needs { reason: Some("you asked me to add the alias".to_owned()), ..Needs::default() };
    let facts = CallFacts {
        targets: vec![("/home/u/.zshrc".into(), Some(TargetKind::File))],
        ..CallFacts::default()
    };
    let requirements =
        shell("echo 'alias k=kubectl' >> ~/.zshrc").with_needs(needs).with_facts(facts);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);

    let info = question(&decision, &requirements, &launch);

    assert_eq!(info.kinds, vec![ExitKind::Persistence]);
    assert_eq!(info.launch, Launch::Unsandboxed);
    assert!(info.grants.is_empty());
    // A target that exists needs no fact: the question names it already.
    assert!(info.facts.is_empty(), "{:?}", info.facts);
    assert_eq!(info.model_reason.as_deref(), Some("you asked me to add the alias"));
    assert!(info.user_only);
    assert_eq!(info.judged, None);
}

#[test]
fn the_question_of_a_network_exit_opens_the_network_by_its_launch() {
    let requirements = shell("npm ci").with_network();
    let decision = decide(&engine(), requirements.clone());
    let launch = grant(&decision);

    let info = question(&decision, &requirements, &launch);

    // The launch says what the grant opens; efr has no fact of its own to add.
    assert_eq!(info.launch, Launch::Contained { grants: vec![Grant::OpenNetwork] });
    assert!(info.facts.is_empty(), "{:?}", info.facts);
    assert!(!info.user_only);
    assert_eq!(info.model_reason, None);
}

fn envelope(event: Event) -> EventEnvelope {
    let clock = TestClock::new();
    EventEnvelope { seq: Seq::ZERO, conversation_id: None, at: Clock::now(&clock), event }
}

#[test]
fn the_user_messages_end_with_the_turn_s_own_prompt() {
    let clock = TestClock::new();
    let turn = |seed| TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(seed)));
    let (first, current, later) = (turn(1), turn(2), turn(3));
    let prompt = |turn_id, text: &str| {
        envelope(Event::PromptQueued {
            turn_id,
            command_id: efr_protocol::CommandId::from_uuid(uuid_v7(&clock, &TestRng::new(4))),
            text: text.to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: efr_protocol::TurnSettings::default(),
            steers: Vec::new(),
        })
    };
    let page = vec![
        prompt(first, "fix the build"),
        envelope(Event::TurnSteered { turn_id: first, text: "only the parser".to_owned() }),
        envelope(Event::TurnCompleted {
            turn_id: first,
            usage: None,
            changes: None,
            context: None,
            stop: None,
            undo: None,
        }),
        prompt(current, "now push it"),
        prompt(later, "and tag it"),
    ];

    assert_eq!(
        user_messages(&page, current),
        vec!["fix the build", "only the parser", "now push it"]
    );
}

#[test]
fn a_withdrawn_prompt_and_a_steer_sent_again_count_once_or_not_at_all() {
    let clock = TestClock::new();
    let turn = |seed| TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(seed)));
    let (first, withdrawn, resent, current) = (turn(1), turn(2), turn(3), turn(5));
    let at = |seq: u64, event: Event| EventEnvelope { seq: Seq::new(seq), ..envelope(event) };
    let prompt = |turn_id, text: &str, steers: Vec<Seq>| Event::PromptQueued {
        turn_id,
        command_id: efr_protocol::CommandId::from_uuid(uuid_v7(&clock, &TestRng::new(4))),
        text: text.to_owned(),
        origin: Origin::Shell,
        context: None,
        settings: efr_protocol::TurnSettings::default(),
        steers,
    };
    let page = vec![
        at(1, prompt(first, "fix the build", Vec::new())),
        at(2, prompt(withdrawn, "never mind", Vec::new())),
        at(3, Event::TurnSteered { turn_id: first, text: "only the parser".to_owned() }),
        at(4, Event::TurnInterruptRequested { turn_id: first, origin: Origin::Shell }),
        at(5, Event::PromptWithdrawn { turn_id: withdrawn, origin: Origin::Shell }),
        at(6, prompt(resent, "only the parser", vec![Seq::new(3)])),
        at(7, prompt(current, "now push it", Vec::new())),
    ];

    assert_eq!(
        user_messages(&page, current),
        vec!["fix the build", "only the parser", "now push it"]
    );
}

#[test]
fn the_user_messages_drop_the_oldest_first() {
    let max = ExitRecord::MAX_USER_MESSAGES_BYTES;
    let old = "o".repeat(max / 2);
    let new = "n".repeat(max / 2);
    let messages = vec!["oldest".to_owned(), old.clone(), new.clone()];
    assert_eq!(bounded(&messages), vec![old, new.clone()]);
    // A newest message that alone is too long keeps its start, cut on a character.
    let long = format!("{}{}", "é".repeat(max / 2), "x");
    let cut = bounded(std::slice::from_ref(&long));
    assert_eq!(cut.len(), 1);
    assert!(cut[0].len() <= max && long.starts_with(&cut[0]), "{}", cut[0].len());
    assert_eq!(bounded(std::slice::from_ref(&new)), vec![new.clone()]);
}

#[test]
fn the_record_holds_the_line_the_facts_and_no_model_reason() {
    let engine = engine();
    let needs =
        Needs { reason: Some("SECRET-REASON".to_owned()), outside: true, ..Needs::default() };
    let facts = CallFacts {
        targets: vec![("/home/u/notes".into(), None)],
        tracked_counts: Vec::new(),
        programs: vec![
            ("make".to_owned(), Some("/usr/bin/make".into()), false),
            ("./build.sh".to_owned(), Some(format!("{APP}/build.sh").into()), true),
        ],
        links: Vec::new(),
        covered: Vec::new(),
        sources: Vec::new(),
        remotes: Vec::new(),
        refused_hosts: Vec::new(),
        real_paths: Vec::new(),
        git_code: None,
    };
    let requirements = shell("./build.sh > ~/notes/today.txt").with_needs(needs).with_facts(facts);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let mut turn = TurnExits::default();
    turn.judged(&[ExitKind::Host], Verdict::Allow);
    turn.ran(&SandboxSummary {
        confined: true,
        promoted: vec!["VIRTUAL_ENV".to_owned()],
        kept_out: vec!["TOKEN".to_owned()],
        ..SandboxSummary::default()
    });
    assert!(!turn.refused());
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };
    let messages = vec!["write the notes to notes/today.txt".to_owned()];

    let record = record(action, &messages, &turn);

    assert_eq!(record.version, ExitRecord::VERSION);
    assert_eq!(record.user_messages, messages);
    assert_eq!(record.action.tool, "shell");
    assert_eq!(record.action.line, "./build.sh > ~/notes/today.txt");
    assert_eq!(record.action.cwd, PathBuf::from(APP));
    assert_eq!(record.action.source, ExitSource::Needs);
    assert_eq!(record.action.exits, kinds(&decision));
    assert!(record.action.exits.contains(&ExitKind::Outside), "{:?}", record.action.exits);
    assert_eq!(record.action.grants, launch.grants());
    let target = record
        .facts
        .targets
        .iter()
        .find(|target| target.path == Path::new("/home/u/notes/today.txt"))
        .expect("the write target");
    assert_eq!(target.class, Some(PathClassName::UserData));
    assert!(!target.in_write_root && !target.floor && !target.synced);
    assert!(target.exists, "no fact about the file itself counts as an existing one");
    assert!(target.named_in_user_messages, "its last two components are in a message");
    let programs: Vec<(&str, bool, bool)> = record
        .facts
        .programs
        .iter()
        .map(|program| (program.word.as_str(), program.in_write_root, program.changed_this_turn))
        .collect();
    assert_eq!(programs, vec![("make", false, false), ("./build.sh", true, true)]);
    assert_eq!(record.facts.sandbox_export_names, vec!["VIRTUAL_ENV", "TOKEN"]);
    assert_eq!(record.facts.previous_exits_this_turn, vec![(ExitKind::Host, Verdict::Allow)]);
    assert_eq!(record.facts.refusals_in_a_row, 1);
    let json = serde_json::to_string(&record).unwrap();
    assert!(!json.contains("SECRET-REASON"), "{json}");
}

#[test]
fn an_upload_names_its_pattern_and_a_host_its_naming() {
    let engine = engine();
    let needs = Needs { hosts: vec!["Example.COM".to_owned()], ..Needs::default() };
    let requirements = shell("git push origin main").with_needs(needs);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };

    let record = record(action, &["push it to example.com".to_owned()], &TurnExits::default());

    assert_eq!(record.facts.upload_patterns, vec!["git push origin main"]);
    assert_eq!(record.facts.hosts.len(), 1);
    assert_eq!(record.facts.hosts[0].host, "example.com");
    assert!(record.facts.hosts[0].named_in_user_messages);
}

/// The live run: `reviewer allowed: uname for this call` for a line whose exit was
/// `df`. The record names the command that leaves the sandbox.
#[test]
fn outside_commands_names_the_command_that_leaves() {
    let engine = engine();
    let requirements = shell("uname -a; df -h");
    let decision = decide(&engine, requirements.clone());
    assert!(decision.exits().any(|need| need.kind == ExitKind::Outside), "{decision:?}");
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };

    let leaving = record(action, &["how full are my disks".to_owned()], &TurnExits::default());

    assert_eq!(leaving.facts.outside_commands, vec!["df -h"]);
    let plain = decide(&engine, shell("uname -a"));
    let action = Action { decision: &plain, ..action };
    assert!(record(action, &[], &TurnExits::default()).facts.outside_commands.is_empty());
}

/// The review of fix batch A: an outside exit that the model asked for with
/// `needs.outside` named the field, so the line said `reviewer allowed: needs.outside`.
#[test]
fn outside_commands_leave_out_the_needs_outside_field() {
    let engine = engine();
    let needs = Needs { outside: true, ..Needs::default() };
    let requirements = shell("uname -a").with_needs(needs);
    let decision = decide(&engine, requirements.clone());
    assert!(
        decision.exits().any(|need| need.kind == ExitKind::Outside && need.part == "needs.outside"),
        "{decision:?}"
    );
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };
    let asked = record(action, &[], &TurnExits::default());
    assert!(asked.facts.outside_commands.is_empty(), "{:?}", asked.facts.outside_commands);
}

#[test]
fn three_refusals_without_a_person_stop_and_an_answer_resets() {
    let mut turn = TurnExits::default();
    assert!(!turn.refused());
    assert!(!turn.refused());
    turn.answered();
    assert!(!turn.refused());
    assert!(!turn.refused());
    assert!(turn.refused(), "the third in a row stops the turn");
}

#[test]
fn only_quarantined_changes_are_asked_about() {
    let change = |path: &str, key: Option<&str>, quarantined: bool| SurfaceChange {
        path: path.into(),
        rule: "rule".to_owned(),
        key: key.map(str::to_owned),
        quarantined,
    };
    let summary = SandboxSummary {
        confined: true,
        surface_changes: vec![
            change("/p/.git/commondir", Some("core.fsmonitor"), true),
            change("/p/.envrc", None, false),
            change("/p/.git/config.worktree", None, true),
        ],
        ..SandboxSummary::default()
    };
    let asked = quarantined(&summary);
    assert_eq!(asked.len(), 2);
    assert_eq!(change_names(&asked), "/p/.git/commondir (core.fsmonitor); /p/.git/config.worktree");
}

#[test]
fn a_path_under_home_is_shown_with_a_tilde() {
    let home = Path::new(HOME);
    assert_eq!(tilde(Path::new("/home/u/.zshrc"), home), "~/.zshrc");
    assert_eq!(tilde(home, home), "~");
    assert_eq!(tilde(Path::new("/etc/hosts"), home), "/etc/hosts");
    assert_eq!(tilde(Path::new("/home/user2/x"), home), "/home/user2/x");
}

#[test]
fn a_write_in_a_shared_directory_says_how_it_binds() {
    let engine = engine();
    let made = CallFacts {
        targets: vec![("/home/u/new.txt".into(), None), ("/home/u".into(), Some(TargetKind::Dir))],
        ..CallFacts::default()
    };
    let requirements = shell("echo x > ~/new.txt").with_facts(made);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let made_first = question(&decision, &requirements, &launch);
    assert_eq!(
        made_first.facts,
        vec![
            "~/new.txt does not exist yet".to_owned(),
            "efr makes the empty file ~/new.txt first".to_owned()
        ]
    );

    let existing = CallFacts {
        targets: vec![("/home/u/.notes".into(), Some(TargetKind::File))],
        ..CallFacts::default()
    };
    let requirements = shell("echo x >> ~/.notes").with_facts(existing);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let in_place = question(&decision, &requirements, &launch);
    assert_eq!(in_place.facts, vec!["in-place writes only; a rename over ~/.notes fails"]);
}

#[test]
fn a_write_grant_of_a_directory_says_that_it_opens_every_file_in_it() {
    let documents = PathBuf::from("/home/u/Documents");
    let facts = CallFacts {
        targets: vec![(documents.clone(), Some(TargetKind::Dir))],
        ..CallFacts::default()
    };
    let requirements = shell("cp report.pdf ~/Documents/").with_write(&documents).with_facts(facts);
    let decision = decide(&engine(), requirements.clone());
    let launch = grant(&decision);
    assert_eq!(launch, Launch::Contained { grants: vec![Grant::Write { path: documents }] });
    let info = question(&decision, &requirements, &launch);
    assert_eq!(info.facts, vec!["~/Documents is a directory: the call can write every file in it"]);
}

/// The question of `line` with `needs.outside`, with `facts` from the daemon.
fn outside_question(line: &str, facts: CallFacts) -> ExitInfo {
    let needs = Needs { outside: true, ..Needs::default() };
    let requirements = shell(line).with_needs(needs).with_facts(facts);
    let decision = decide(&engine(), requirements.clone());
    let launch = grant(&decision);
    question(&decision, &requirements, &launch)
}

#[test]
fn outside_with_only_a_bus_exit_says_that_the_bus_covers_the_line() {
    let info = outside_question("systemctl --failed --no-pager", CallFacts::default());
    // The model's request stands: the user decides, and the line runs outside on a yes.
    assert_eq!(info.kinds, vec![ExitKind::Bus, ExitKind::Outside]);
    assert_eq!(info.launch, Launch::Unsandboxed);
    assert_eq!(
        info.facts,
        vec![
            "a grant of the system bus covers what the line shows; the model asked for outside"
                .to_owned()
        ]
    );
}

#[test]
fn outside_with_only_a_write_exit_names_the_one_path() {
    let facts = CallFacts {
        targets: vec![
            ("/home/u/efr-note.txt".into(), None),
            ("/home/u".into(), Some(TargetKind::Dir)),
        ],
        ..CallFacts::default()
    };
    let info = outside_question("echo hi > ~/efr-note.txt", facts);
    assert_eq!(info.launch, Launch::Unsandboxed);
    assert_eq!(
        info.facts.last().map(String::as_str),
        Some(
            "a grant of ~/efr-note.txt writable covers what the line shows; the model asked \
             for outside"
        ),
        "{:?}",
        info.facts
    );
}

#[test]
fn outside_for_a_line_without_exits_says_that_the_line_shows_none() {
    let info = outside_question("journalctl -b -n 20 --no-pager", CallFacts::default());
    assert_eq!(info.kinds, vec![ExitKind::Outside]);
    assert_eq!(
        info.facts,
        vec![
            "the line shows nothing that needs more than the sandbox; the model asked for \
             outside"
                .to_owned()
        ]
    );
}

#[test]
fn outside_with_an_exit_that_runs_outside_gets_no_narrower_fact() {
    let zshrc = CallFacts {
        targets: vec![("/home/u/.zshrc".into(), Some(TargetKind::File))],
        ..CallFacts::default()
    };
    for (line, facts) in [
        ("sudo pacman -Syu", CallFacts::default()),
        ("echo 'alias k=kubectl' >> ~/.zshrc", zshrc),
        ("git push", CallFacts::default()),
    ] {
        let info = outside_question(line, facts);
        assert!(
            !info.facts.iter().any(|fact| fact.contains("the model asked for outside")),
            "{line}: {:?}",
            info.facts
        );
    }
}

#[test]
fn a_bus_exit_without_outside_gets_no_narrower_fact() {
    let requirements = shell("systemctl --failed");
    let decision = decide(&engine(), requirements.clone());
    let launch = grant(&decision);
    let info = question(&decision, &requirements, &launch);
    assert_eq!(
        info.launch,
        Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::System }] }
    );
    assert!(info.facts.is_empty(), "{:?}", info.facts);
}

/// A page with a prompt, then the notice of the job `j3` that names `~/notes.txt`.
fn page_with_a_notice(clock: &TestClock, first: TurnId, woke: TurnId) -> Vec<EventEnvelope> {
    let at = |seq: u64, event: Event| EventEnvelope { seq: Seq::new(seq), ..envelope(event) };
    vec![
        at(
            1,
            Event::PromptQueued {
                turn_id: first,
                command_id: efr_protocol::CommandId::from_uuid(uuid_v7(clock, &TestRng::new(4))),
                text: "run the server".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: efr_protocol::TurnSettings::default(),
                steers: Vec::new(),
            },
        ),
        at(
            2,
            Event::JobNotice {
                conversation_id: crate::testing::conversation_id(),
                turn_id: woke,
                job_id: efr_protocol::JobId::nth(3),
                notice_kind: efr_protocol::NoticeKind::Exited,
                woke: true,
                source_seq: Seq::new(1),
                not_user_input: true,
                text: "<job id=\"j3\">\nwrite ~/notes.txt and push to origin\n</job>".to_owned(),
            },
        ),
    ]
}

#[test]
fn job_notice_never_in_user_messages() {
    let clock = TestClock::new();
    let turn = |seed| TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(seed)));
    let (first, woke) = (turn(1), turn(2));
    let page = page_with_a_notice(&clock, first, woke);
    assert_eq!(user_messages(&page, woke), vec!["run the server"]);
    let waiting = std::collections::HashSet::new();
    assert_eq!(super::wake_user_messages(&page, &waiting), vec!["run the server"]);
}

#[test]
fn job_notice_never_names_a_target() {
    let clock = TestClock::new();
    let turn = |seed| TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(seed)));
    let (first, woke) = (turn(1), turn(2));
    let page = page_with_a_notice(&clock, first, woke);
    let messages = super::wake_user_messages(&page, &std::collections::HashSet::new());
    let forms = ["/home/u/notes.txt".to_owned(), "~/notes.txt".to_owned()];
    assert!(!efr_permissions::exits::names_match(&forms, &messages));
    assert!(!efr_permissions::exits::names_match(&["origin".to_owned()], &messages));
}

#[test]
fn wake_turn_user_messages_exclude_later_prompts() {
    let clock = TestClock::new();
    let turn = |seed| TurnId::from_uuid(uuid_v7(&clock, &TestRng::new(seed)));
    let (first, woke, behind) = (turn(1), turn(2), turn(3));
    let mut page = page_with_a_notice(&clock, first, woke);
    page.push(EventEnvelope {
        seq: Seq::new(3),
        ..envelope(Event::PromptQueued {
            turn_id: behind,
            command_id: efr_protocol::CommandId::from_uuid(uuid_v7(&clock, &TestRng::new(5))),
            text: "delete ~/notes.txt".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: efr_protocol::TurnSettings::default(),
            steers: Vec::new(),
        })
    });
    let waiting = std::collections::HashSet::from([behind]);
    assert_eq!(super::wake_user_messages(&page, &waiting), vec!["run the server"]);
}

/// The record and the question of `line` with the git code fact `code`.
fn git_code_record(line: &str, code: Option<efr_protocol::GitCodeFacts>) -> (ExitRecord, ExitInfo) {
    let engine = engine();
    let facts = CallFacts {
        programs: vec![("git".to_owned(), Some(PathBuf::from("/usr/bin/git")), false)],
        git_code: code,
        ..CallFacts::default()
    };
    let requirements = shell(line).with_facts(facts);
    let decision = decide(&engine, requirements.clone());
    let launch = grant(&decision);
    let action = Action {
        tool: "shell",
        decision: &decision,
        requirements: &requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine: &engine,
        payload: None,
    };
    let record = record(action, &["push it".to_owned()], &TurnExits::default());
    (record, question(&decision, &requirements, &launch))
}

#[test]
fn writable_git_code_reaches_the_record_and_the_question() {
    let hook = format!("{SCRATCH}/r/.git/hooks/pre-push");
    let code = efr_protocol::GitCodeFacts {
        checked: true,
        items: vec![efr_protocol::GitCodeFact {
            repo: format!("{SCRATCH}/r").into(),
            item: "hooks/pre-push".to_owned(),
            file: hook.into(),
            writable: true,
        }],
    };
    let line = "git push https://git.test/r.git main";
    let (record, info) = git_code_record(line, Some(code.clone()));
    assert_eq!(record.facts.git_code, Some(code));
    assert!(info.user_only);
    assert_eq!(
        info.facts,
        ["git runs ~/.local/share/efr/scratch/2026-10-04-fix-0a1b2c3d/r/.git/hooks/pre-push, \
             which a sandboxed command can write"]
    );
}

#[test]
fn a_clean_git_code_check_stays_out_of_the_record() {
    // NOTE: so the reviewer never reads a clean check as a reason to allow.
    let line = "git push https://git.test/r.git main";
    let clean = efr_protocol::GitCodeFacts { checked: true, items: Vec::new() };
    let (record, info) = git_code_record(line, Some(clean));
    assert_eq!(record.facts.git_code, None);
    assert!(info.facts.is_empty(), "{:?}", info.facts);
    // A line that leaves nothing outside records none either.
    let late = efr_protocol::GitCodeFacts { checked: false, items: Vec::new() };
    let (record, _) = git_code_record("git status", Some(late.clone()));
    assert_eq!(record.facts.git_code, None);
    let (record, info) = git_code_record(line, Some(late.clone()));
    assert_eq!(record.facts.git_code, Some(late));
    assert_eq!(info.facts, ["efr could not check the git code of ~/p/app"]);
}

#[test]
fn a_publish_question_says_what_it_runs() {
    let (_, info) = git_code_record("npm publish", None);
    assert!(info.user_only);
    assert_eq!(info.facts, ["runs the project's prepublishOnly and prepack scripts"]);
}
