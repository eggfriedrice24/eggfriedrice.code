use std::path::{Path, PathBuf};

use efr_permissions::{
    CallFacts, ConversationPolicy, Decision, DecisionInput, Engine, Locations, Requirements,
    TargetKind,
};
use efr_protocol::{
    BusKind, Event, EventEnvelope, ExitKind, ExitRecord, ExitSource, Grant, Launch, Mode, Needs,
    Origin, PathClassName, ProjectId, SandboxSummary, Scope, Seq, SurfaceChange, TurnId, Verdict,
};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Clock;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{
    Action, TurnExits, bounded, change_names, floor_kinds, grant, info, kinds, quarantined, record,
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

    let info = info(&decision, &requirements, &launch, Path::new(HOME));

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

    let info = info(&decision, &requirements, &launch, Path::new(HOME));

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
        })
    };
    let page = vec![
        prompt(first, "fix the build"),
        envelope(Event::TurnSteered { turn_id: first, text: "only the parser".to_owned() }),
        envelope(Event::TurnCompleted { turn_id: first, usage: None }),
        prompt(current, "now push it"),
        prompt(later, "and tag it"),
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
    };

    let record = record(action, &["push it to example.com".to_owned()], &TurnExits::default());

    assert_eq!(record.facts.upload_patterns, vec!["git push origin main"]);
    assert_eq!(record.facts.hosts.len(), 1);
    assert_eq!(record.facts.hosts[0].host, "example.com");
    assert!(record.facts.hosts[0].named_in_user_messages);
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
    let made_first = info(&decision, &requirements, &launch, Path::new(HOME));
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
    let in_place = info(&decision, &requirements, &launch, Path::new(HOME));
    assert_eq!(in_place.facts, vec!["in-place writes only; a rename over ~/.notes fails"]);
}

/// The question of `line` with `needs.outside`, with `facts` from the daemon.
fn outside_question(line: &str, facts: CallFacts) -> efr_protocol::ExitInfo {
    let needs = Needs { outside: true, ..Needs::default() };
    let requirements = shell(line).with_needs(needs).with_facts(facts);
    let decision = decide(&engine(), requirements.clone());
    let launch = grant(&decision);
    info(&decision, &requirements, &launch, Path::new(HOME))
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
    let info = info(&decision, &requirements, &launch, Path::new(HOME));
    assert_eq!(
        info.launch,
        Launch::Contained { grants: vec![Grant::Bus { bus: BusKind::System }] }
    );
    assert!(info.facts.is_empty(), "{:?}", info.facts);
}
