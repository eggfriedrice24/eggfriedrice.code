//! The command and attack corpus through the engine in the `auto` mode: every line
//! gets the phase A result of its `pa` column, on a machine with undo the phase B
//! result of its `pb` column, and on a machine with the network view and the mount view
//! the phase D1 result of its `pd` column, which is the `pb` result where the line has
//! none (`tests/fixtures/auto-corpus.toml`). With efr's network proxy it gets the phase
//! D2 result of `pd2`, and with the filtered bus proxy the phase E result of `pe`, each
//! the earlier result where the line has none. The `p1` column keeps the phase 1 result
//! of spec Appendix A. In full mode every line gets the result of its `full` column,
//! which a line has only where the result differs from the one that its phase D1 result
//! gives (efr's full mode, DF25).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use efr_permissions::exits::fact_requests;
use efr_permissions::{
    AllowRule, AlwaysVerdict, Approver, AutoSupport, CallFacts, Cause, ConversationPolicy,
    CoverAsk, Coverage, Decision, DecisionInput, Effect, Egress, Engine, HostList, Locations,
    Policy, Preset, RemoteFact, RemoteUrl, Requirements, SettingsChange, Source, SourceFact,
    Subject, TargetKind,
};
use efr_protocol::{BusKind, ExitKind, GitCodeFacts, Grant, Mode, Origin, ProjectId, Scope};
use pretty_assertions::assert_eq;
use serde::Deserialize;

const CORPUS: &str = include_str!("../fixtures/auto-corpus.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    line: Vec<Line>,
}

/// One line of the fixture; its header says what each key means.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    n: u32,
    #[serde(default)]
    line: Option<String>,
    #[serde(default)]
    settings: Option<String>,
    p1: String,
    #[serde(default)]
    engine: Option<String>,
    pa: String,
    pb: String,
    /// The phase D1 result, when it differs from `pb`.
    #[serde(default)]
    pd: Option<String>,
    /// The phase D2 result, when it differs from the phase D1 result.
    #[serde(default)]
    pd2: Option<String>,
    /// The phase E result, when it differs from the phase D2 result.
    #[serde(default)]
    pe: Option<String>,
    /// The full mode result, when it differs from the one that [`full_default`] gives.
    #[serde(default)]
    full: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    covered: Vec<String>,
    #[serde(default)]
    left_out: Vec<String>,
    #[serde(default)]
    persist: Vec<String>,
    #[serde(default)]
    sources: Vec<String>,
    exits: Vec<ExitKind>,
    #[serde(default)]
    reads: Vec<PathBuf>,
    #[serde(default)]
    trees: Vec<PathBuf>,
    #[serde(default)]
    writes: Vec<PathBuf>,
    #[serde(default)]
    network: bool,
    #[serde(default)]
    interactive: bool,
    #[serde(default)]
    missing: Vec<PathBuf>,
    #[serde(default)]
    dirs: Vec<PathBuf>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    tracked: Vec<Tracked>,
    /// The remotes that efrd reads for the project, each `name=host`: one https URL
    /// for fetch and push.
    #[serde(default)]
    remotes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tracked {
    path: PathBuf,
    count: u32,
}

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/efr";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-06-corpus-0a1b2c3d";

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// The machine of the fixture's header, with the home store (phase A).
fn engine() -> Engine {
    machine(AutoSupport { floor_backup: true, ..AutoSupport::default() })
}

/// The machine of the fixture's header with snapshots and undo (phase B), and
/// `sandbox.ask_destructive = "unless_undoable"`.
fn undo_engine() -> Engine {
    machine(AutoSupport { floor_backup: true, undo: true, ..AutoSupport::default() })
}

/// The machine of the fixture's header in phase D1: undo and the home store as in phase
/// B, the network view and the mount view.
fn pd_engine() -> Engine {
    machine(AutoSupport {
        netview: true,
        mount_view: true,
        undo: true,
        floor_backup: true,
        ..AutoSupport::default()
    })
}

/// The phase D1 result of `line`: its `pd` column, else its `pb` column.
fn pd(line: &Line) -> &str {
    line.pd.as_deref().unwrap_or(&line.pb)
}

/// The machine of phase D2: the phase D1 machine with efr's network proxy and the
/// default allow list of spec 8.3 (the six presets, no pacman mirror).
fn pd2_engine() -> Engine {
    let allow: Vec<String> = Preset::ALL.iter().map(|preset| preset.name().to_owned()).collect();
    let hosts = HostList::compile(&allow, &[], &[], &[]).unwrap();
    machine(AutoSupport {
        egress: Egress::Proxy,
        netview: true,
        mount_view: true,
        undo: true,
        floor_backup: true,
        ..AutoSupport::default()
    })
    .with_hosts(Arc::new(hosts))
}

/// The phase D2 result of `line`: its `pd2` column, else its phase D1 result.
fn pd2(line: &Line) -> &str {
    line.pd2.as_deref().unwrap_or_else(|| pd(line))
}

/// The machine of phase E: the phase D2 machine with the filtered bus proxy.
fn pe_engine() -> Engine {
    let support = AutoSupport { bus_proxy: true, ..*pd2_engine().support() };
    pd2_engine().with_support(support)
}

/// The phase E result of `line`: its `pe` column, else its phase D2 result.
fn pe(line: &Line) -> &str {
    line.pe.as_deref().unwrap_or_else(|| pd2(line))
}

/// The machine of full mode: the phase E machine. Full mode reads only its `undo` and
/// its home store; it decides as if no sandbox ran (`AutoSupport::for_full`), which is
/// the support of the phase D1 machine.
fn full_engine() -> Engine {
    pe_engine()
}

/// The full mode result that the phase D1 result of `line` gives, in the words of the
/// `full` column: a line that runs with no exit runs (`R`); an exit runs at once and is
/// recorded (`A(kind)`), but a `privilege`, `persistence` or `destructive` exit asks the
/// user (`U(kind)`); a floor stays a floor (`D`), and the settings floor asks.
fn full_default(line: &Line) -> String {
    let auto = pd(line);
    if matches!(auto, "R" | "C") {
        return "R".to_owned();
    }
    let kind = auto
        .strip_prefix(['K', 'N', 'U'])
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'));
    match kind {
        Some(kind @ ("privilege" | "persistence" | "destructive")) => format!("U({kind})"),
        Some(kind) => format!("A({kind})"),
        None => auto.to_owned(),
    }
}

/// The full mode result of `line`: its `full` column, else [`full_default`].
fn full(line: &Line) -> String {
    line.full.clone().unwrap_or_else(|| full_default(line))
}

/// The machine of the fixture's header, whose sandbox supports `support`.
fn machine(support: AutoSupport) -> Engine {
    let mut locations = Locations::new(HOME)
        .unwrap()
        .with_sealed_root("/home/u/.local/share/efr/secrets")
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap()
        .with_synced_root("/home/u/Dropbox")
        .unwrap()
        .with_project(id(1), PROJECT)
        .unwrap()
        .with_project(id(2), "/home/u/p/app")
        .unwrap()
        .with_project(id(3), "/home/u/p/project-x")
        .unwrap();
    for root in ["/tmp", "/var/tmp", "/dev/shm"] {
        locations = locations.with_private_root(root).unwrap();
    }
    for root in [
        "/home/u/.cargo",
        "/home/u/.rustup",
        "/home/u/.cache",
        "/home/u/go/pkg/mod",
        "/home/u/.npm",
    ] {
        locations = locations.with_cache_root(root).unwrap();
    }
    for root in ["/home/u/p/app", "/home/u/p/project-x"] {
        locations = locations.with_envelope_root(root).unwrap();
    }
    Engine::with_rules(locations, Policy::empty()).with_support(support)
}

/// One coverage answer of the fixture, such as `path:/home/u/p/efr/a.log`.
fn ask(form: &str) -> CoverAsk {
    let (kind, path) = form.split_once(':').unwrap_or_else(|| panic!("no kind in {form:?}"));
    let path = PathBuf::from(path);
    match kind {
        "path" => CoverAsk::Path(path),
        "tracked" => CoverAsk::Tracked(path),
        "untracked" => CoverAsk::Untracked(path),
        "refs" => CoverAsk::Refs(path),
        "stash" => CoverAsk::Stash(path),
        "home" => CoverAsk::Home(path),
        other => panic!("unknown kind {other:?} in {form:?}"),
    }
}

/// One answer of the fixture's `persist` key: what efrd can copy before a named unit,
/// cron or linger change, `unit:<name>`, `cron` or `linger`.
fn persist_ask(form: &str) -> CoverAsk {
    match form.split_once(':') {
        Some(("unit", name)) => CoverAsk::UnitLinks(name.to_owned()),
        None if form == "cron" => CoverAsk::CronTable,
        None if form == "linger" => CoverAsk::Linger(None),
        _ => panic!("unknown persist form {form:?}"),
    }
}

/// One fact of the fixture's `sources` key: a file that the line installs,
/// `file:<path>`, or the unit file of a unit, `unit:<name>=<path>`, each a small text
/// file outside every write root that did not change in the turn.
fn source_fact(form: &str) -> SourceFact {
    let (source, path) = match form.split_once(':') {
        Some(("file", path)) => (Source::File(PathBuf::from(path)), PathBuf::from(path)),
        Some(("unit", rest)) => {
            let (name, path) =
                rest.split_once('=').unwrap_or_else(|| panic!("no path in {form:?}"));
            (Source::Unit(name.to_owned()), PathBuf::from(path))
        }
        _ => panic!("unknown source form {form:?}"),
    };
    SourceFact {
        source,
        path: Some(path),
        changed: false,
        text: Some("[Install]\nWantedBy=default.target\n".to_owned()),
    }
}

/// What the call of `line` declares, with the facts the daemon would collect: in phase
/// B also its answers to the coverage asks. `phase_b` adds the facts that only a phase
/// B efrd collects for named persistence (`persist`, `sources`).
fn requirements(line: &Line, phase_b: bool) -> Requirements {
    if let Some(summary) = &line.settings {
        return Requirements::none().with_settings_change(SettingsChange::new(summary, true));
    }
    let text = line.line.clone().unwrap();
    let mut requirements = Requirements::none().with_command(&text).with_command_dir(PROJECT);
    for path in &line.reads {
        requirements = requirements.with_read(path);
    }
    for path in &line.trees {
        requirements = requirements.with_read_tree(path);
    }
    for path in &line.writes {
        requirements = requirements.with_write(path);
    }
    if line.network {
        requirements = requirements.with_network();
    }
    if line.interactive {
        requirements = requirements.with_interactive();
    }
    let kinds = [
        (&line.missing, None),
        (&line.dirs, Some(TargetKind::Dir)),
        (&line.files, Some(TargetKind::File)),
    ];
    let targets = kinds
        .into_iter()
        .flat_map(|(paths, kind)| paths.iter().map(move |path| (path.clone(), kind)))
        .collect();
    let tracked_counts =
        line.tracked.iter().map(|tracked| (tracked.path.clone(), tracked.count)).collect();
    let locations = Locations::new(HOME).unwrap();
    let request = fact_requests(&text, Some(PROJECT.as_ref()), &locations);
    let programs = request
        .programs
        .iter()
        .map(|word| {
            let resolved = (!request.builtins.contains(word))
                .then(|| PathBuf::from("/usr/bin").join(word.rsplit('/').next().unwrap()));
            (word.clone(), resolved, false)
        })
        .collect();
    let covered = line
        .covered
        .iter()
        .map(|form| (ask(form), Coverage::Covered))
        .chain(line.left_out.iter().map(|form| (ask(form), Coverage::LeftOut)))
        .chain(
            line.persist
                .iter()
                .filter(|_| phase_b)
                .map(|form| (persist_ask(form), Coverage::Covered)),
        )
        .collect();
    let sources = line.sources.iter().filter(|_| phase_b).map(|form| source_fact(form)).collect();
    requirements.with_facts(CallFacts {
        targets,
        tracked_counts,
        programs,
        links: Vec::new(),
        covered,
        sources,
        remotes: remote_facts(&line.remotes),
        refused_hosts: Vec::new(),
        real_paths: Vec::new(),
        // NOTE: the project's git code lies in its pinned git dir, which no contained
        // call writes (v0.0.4, DV22).
        git_code: (!request.git_code.is_empty())
            .then(|| GitCodeFacts { checked: true, items: Vec::new() }),
    })
}

/// The remote facts of the project for the `name=host` words of a corpus line.
fn remote_facts(words: &[String]) -> Vec<(PathBuf, Vec<RemoteFact>)> {
    if words.is_empty() {
        return Vec::new();
    }
    let remotes = words
        .iter()
        .map(|word| {
            let (name, host) = word.split_once('=').unwrap();
            let url =
                RemoteUrl { host: host.to_owned(), port: 443, ssh: false, credentials: false };
            RemoteFact {
                name: name.to_owned(),
                fetch: vec![url.clone()],
                push: vec![url],
                project: false,
                upstream: false,
                submodule: false,
            }
        })
        .collect();
    vec![(PathBuf::from(PROJECT), remotes)]
}

/// The decision input of `line` in a turn of the project, with the facts of phase B.
fn input(line: &Line) -> DecisionInput {
    DecisionInput {
        requirements: requirements(line, true),
        scope: Scope::Project(id(1)),
        origin: Origin::Shell,
        mode: Mode::Auto,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    }
}

/// The lines of the corpus whose result on `engine` is not the one of `column`, each
/// with what it found. A line that runs with no exit (`R` or `C`) must have none; any
/// other line must have the exits of the fixture.
fn mismatches(engine: &Engine, phase_b: bool, column: impl Fn(&Line) -> &str) -> Vec<String> {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let mut failures = Vec::new();
    for line in &corpus.line {
        let expected = column(line);
        let input = DecisionInput {
            requirements: requirements(line, phase_b),
            scope: Scope::Project(id(1)),
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(SCRATCH),
        };
        let decision = engine.decide(&input);
        let found = verdict(&decision, expected);
        let exits: BTreeSet<ExitKind> = decision.exits().map(|need| need.kind).collect();
        let wanted: BTreeSet<ExitKind> = if matches!(expected, "R" | "C") && expected != line.pa {
            BTreeSet::new()
        } else {
            line.exits.iter().copied().collect()
        };
        let text = line.line.as_deref().or(line.settings.as_deref()).unwrap_or_default();
        if found != expected || exits != wanted {
            failures.push(format!(
                "line {} {text:?}: expected {expected} with {wanted:?}, found {found} with {exits:?}",
                line.n
            ));
        }
    }
    failures
}

/// The lines of the corpus whose result in full mode is not the one of the `full`
/// column, each with what it found. A line that runs with no exit in phase D1 has none
/// in full mode; any other line has the exits of the fixture.
fn full_mismatches(engine: &Engine) -> Vec<String> {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let mut failures = Vec::new();
    for line in &corpus.line {
        let expected = full(line);
        let input = DecisionInput { mode: Mode::Full, ..input(line) };
        let decision = engine.decide(&input);
        let found = full_verdict(&decision, &expected);
        let exits: BTreeSet<ExitKind> = decision.exits().map(|need| need.kind).collect();
        let wanted: BTreeSet<ExitKind> = if matches!(pd(line), "R" | "C") {
            BTreeSet::new()
        } else {
            line.exits.iter().copied().collect()
        };
        let text = line.line.as_deref().or(line.settings.as_deref()).unwrap_or_default();
        if found != expected || exits != wanted {
            failures.push(format!(
                "line {} {text:?}: expected {expected} with {wanted:?}, found {found} with {exits:?}",
                line.n
            ));
        }
    }
    failures
}

/// The kind that `expected` names, such as `host` in `A(host)`.
fn named_kind(expected: &str) -> &str {
    expected
        .strip_prefix(['A', 'K', 'N', 'U'])
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_default()
}

/// The result of a full mode `decision` in the words of the `full` column: `R` for a
/// line that runs with no exit, `A(kind)` for one that runs with its exits recorded,
/// `G` for a question about a delete that full mode guards, `S` for a question about a
/// directory with a secret or efr's config below it, `U(kind)` for a question about an
/// exit, `D` for a floor.
fn full_verdict(decision: &Decision, expected: &str) -> String {
    let named = named_kind(expected);
    let pick = |kinds: Vec<ExitKind>| {
        kinds.iter().map(|kind| kind.as_str()).find(|kind| *kind == named).unwrap_or("none of them")
    };
    match decision.effect() {
        Effect::Deny => "D".to_owned(),
        Effect::Contain => "contain".to_owned(),
        Effect::Allow if decision.exits().next().is_none() => "R".to_owned(),
        Effect::Allow => format!("A({})", pick(decision.exits().map(|need| need.kind).collect())),
        Effect::Ask
            if decision.reasons().iter().any(|reason| reason.cause == Cause::SettingsChange) =>
        {
            "asks (settings floor)".to_owned()
        }
        Effect::Ask if decision.critical().next().is_some() => "G".to_owned(),
        Effect::Ask
            if decision.reasons().iter().any(|reason| {
                reason.effect == Effect::Ask
                    && matches!(
                        reason.cause,
                        Cause::ReachesSecret { .. } | Cause::ReachesWriteSealed { .. }
                    )
            }) =>
        {
            "S".to_owned()
        }
        Effect::Ask => {
            let asking = decision
                .reasons()
                .iter()
                .filter(|reason| reason.effect == Effect::Ask)
                .filter_map(|reason| match &reason.subject {
                    Subject::Exit { need } => Some(need.kind),
                    _ => None,
                })
                .collect();
            format!("U({})", pick(asking))
        }
    }
}

/// The result of `decision` in the words of the `pa` column: `R` for a line that runs
/// contained with no exit (the corpus's `C` lines too), `K(kind)`, `N(kind)` and
/// `U(kind)` for an exit that asks, by the strictest approver of the call, and `D` for
/// a floor.
fn verdict(decision: &Decision, expected: &str) -> String {
    match decision.effect() {
        Effect::Deny => "D".to_owned(),
        Effect::Allow => "allow".to_owned(),
        Effect::Contain if decision.exits().next().is_none() => {
            if expected == "C" {
                "C".to_owned()
            } else {
                "R".to_owned()
            }
        }
        Effect::Contain => "contain with exits".to_owned(),
        Effect::Ask
            if decision.reasons().iter().any(|reason| reason.cause == Cause::SettingsChange) =>
        {
            "asks (settings floor)".to_owned()
        }
        Effect::Ask => {
            let approver = decision.exits().map(|need| need.approver).max();
            let letter = match approver {
                Some(Approver::User) => "U",
                Some(Approver::ClassifierWhenNamed) => "N",
                Some(Approver::Classifier) => "K",
                None => "none",
            };
            // NOTE: the kind that the column names must be an exit with that approver;
            // the fixture's `exits` holds the full set.
            let named = expected
                .strip_prefix(['K', 'N', 'U'])
                .and_then(|rest| rest.strip_prefix('('))
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or_default();
            let kind = decision
                .exits()
                .filter(|need| Some(need.approver) == approver)
                .map(|need| need.kind.as_str())
                .find(|kind| *kind == named)
                .unwrap_or("none of them");
            format!("{letter}({kind})")
        }
    }
}

/// How strict a phase 1 result is: a floor over a user-only exit over an exit over a
/// routine or contained line.
fn rank(result: &str) -> u8 {
    match result.chars().next() {
        Some('D') => 3,
        Some('U') => 2,
        Some('X') => 1,
        _ => 0,
    }
}

#[test]
fn every_line_has_the_phase_a_result() {
    assert_eq!(mismatches(&engine(), false, |line| &line.pa), Vec::<String>::new());
}

#[test]
fn every_line_has_the_phase_b_result() {
    assert_eq!(mismatches(&undo_engine(), true, |line| &line.pb), Vec::<String>::new());
}

#[test]
fn pd_corpus_rows_match() {
    assert_eq!(mismatches(&pd_engine(), true, pd), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let changed: Vec<u32> =
        corpus.line.iter().filter(|line| line.pd.is_some()).map(|line| line.n).collect();
    assert_eq!(changed, [13, 41], "only the df lines differ from phase B");
}

#[test]
fn pd2_corpus_rows_match() {
    assert_eq!(mismatches(&pd2_engine(), true, pd2), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let changed: BTreeSet<u32> =
        corpus.line.iter().filter(|line| line.pd2.is_some()).map(|line| line.n).collect();
    assert_eq!(changed, BTreeSet::from([18, 25, 28, 35, 66, 101]));
}

#[test]
fn routine_lines_stay_routine_in_phase_d2() {
    // A line that runs with no exit in phase D1 makes no exit in phase D2 either; phase D2
    // only takes `host` exits away, where the proxy decides the fetch.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        if matches!(pd(line), "R" | "C") {
            assert_eq!(pd2(line), pd(line), "line {} {:?}", line.n, line.line);
        }
        if let Some(pd2) = &line.pd2 {
            assert_ne!(pd2, pd(line), "line {}: a `pd2` that equals `pd` says nothing", line.n);
            assert!(matches!(pd2.as_str(), "R" | "C"), "line {} {:?}", line.n, line.line);
            assert_eq!(pd(line), "K(host)", "line {} {:?}", line.n, line.line);
        }
    }
}

#[test]
fn pe_corpus_rows_match() {
    assert_eq!(mismatches(&pe_engine(), true, pe), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let changed: BTreeSet<u32> =
        corpus.line.iter().filter(|line| line.pe.is_some()).map(|line| line.n).collect();
    assert_eq!(changed, BTreeSet::from([8, 9, 10, 11]), "rows 8 to 11 become routine");
    // Each of them reads the filtered bus of its view.
    let engine = pe_engine();
    for line in corpus.line.iter().filter(|line| line.pe.is_some()) {
        let decision = engine.decide(&input(line));
        let bus = if line.line.as_deref().is_some_and(|text| text.contains("--user")) {
            BusKind::Session
        } else {
            BusKind::System
        };
        let views: Vec<&Grant> = decision.view_grants().collect();
        assert_eq!(views, [&Grant::BusFilter { bus }], "line {} {:?}", line.n, line.line);
    }
}

#[test]
fn routine_lines_stay_routine_in_phase_e() {
    // A line that runs with no exit in phase D2 makes no exit in phase E either; phase E
    // only takes `bus` exits away, where a read form reads the filtered bus.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        if matches!(pd2(line), "R" | "C") {
            assert_eq!(pe(line), pd2(line), "line {} {:?}", line.n, line.line);
        }
        if let Some(pe) = &line.pe {
            assert_ne!(pe, pd2(line), "line {}: a `pe` that equals `pd2` says nothing", line.n);
            assert_eq!(pe, "R", "line {} {:?}", line.n, line.line);
            assert_eq!(pd2(line), "K(bus)", "line {} {:?}", line.n, line.line);
        }
    }
}

/// Always rules that name the resources of the corpus lines that must still ask (79,
/// 80, 85, 89), and some that a line could match.
fn named_rules() -> Vec<AllowRule> {
    let socket = |path: &str| AllowRule::Socket { path: path.to_owned() };
    let write = |under: &str| AllowRule::Write { under: under.to_owned(), project: None };
    vec![
        socket("/run/dbus/system_bus_socket"),
        socket("/run/user/1000/systemd/private"),
        socket("/run/systemd/private"),
        write("~/.config/systemd/user"),
        write("~/.local/share/systemd"),
        write("~/notes"),
        write("~/Documents"),
        AllowRule::Host { host: "example.com".to_owned(), port: 443 },
        AllowRule::Host { host: "github.com".to_owned(), port: 443 },
    ]
}

#[test]
fn corpus_79_80_85_89_ask_with_bus_proxy_and_rules() {
    let engine = pe_engine().with_allow(named_rules());
    let disabled: Vec<bool> =
        engine.allow_rules().iter().map(|(_, floor)| floor.is_some()).collect();
    assert_eq!(disabled, [true, true, true, true, true, false, false, false, false]);
    assert_eq!(mismatches(&engine, true, pe), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        let decision = engine.decide(&input(line));
        let verdict = engine.always(&decision, Some(PROJECT.as_ref()));
        if [79, 80, 85, 89].contains(&line.n) {
            assert_eq!(decision.effect(), Effect::Ask, "line {} {:?}", line.n, line.line);
            assert_eq!(verdict, AlwaysVerdict::None, "line {} {:?}", line.n, line.line);
        }
        // A covered or offered line asks only for exits that a rule may cover.
        if verdict != AlwaysVerdict::None {
            for need in decision.exits() {
                assert!(
                    matches!(need.kind, ExitKind::Host | ExitKind::Write | ExitKind::Socket)
                        && need.approver == Approver::Classifier
                        && !need.runs_unsandboxed()
                        && need.grants.iter().all(|grant| {
                            matches!(
                                grant,
                                Grant::Host { .. } | Grant::Write { .. } | Grant::Socket { .. }
                            )
                        }),
                    "line {} {:?}: {need:?}",
                    line.n,
                    line.line
                );
            }
        }
    }
}

#[test]
fn ask_destructive_always_keeps_every_destructive_exit() {
    // `sandbox.ask_destructive = "always"` (or `undo.enabled = false`) keeps the phase
    // A route of every line, whatever the snapshot covers.
    assert_eq!(mismatches(&engine(), false, |line| &line.pa), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let covered = corpus.line.iter().filter(|line| !line.covered.is_empty()).count();
    assert!(covered >= 3, "the corpus has covered lines: {covered}");
}

#[test]
fn the_fixture_holds_all_110_lines_and_only_stricter_overrides() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let numbers: BTreeSet<u32> = corpus.line.iter().map(|line| line.n).collect();
    assert_eq!(numbers, (1..=110).collect::<BTreeSet<u32>>());
    for line in &corpus.line {
        assert!(line.line.is_some() != line.settings.is_some(), "line {}", line.n);
        // NOTE: only a phase B efrd collects the facts of named persistence.
        if !line.persist.is_empty() || !line.sources.is_empty() {
            assert!(line.pb.starts_with('N') || line.pb.starts_with('U'), "line {}", line.n);
        }
        if let Some(engine) = &line.engine {
            assert!(
                rank(engine) > rank(&line.p1),
                "line {}: {engine} is looser than {}",
                line.n,
                line.p1
            );
            assert!(line.note.is_some(), "line {}: an override says why", line.n);
        }
    }
}

#[test]
fn routine_lines_stay_routine_in_phase_a() {
    // The gate of spec 16.3: a line that ran with no exit in phase 1 makes no exit
    // now, so it costs no classifier call. `df` is the one change: it shows the
    // sandbox's mounts until the mount view exists, so it runs outside after a review.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        let df = line.line.as_deref().is_some_and(|text| text.starts_with("df "));
        if matches!(line.p1.as_str(), "R" | "C") && !df {
            assert_eq!(line.pa, line.p1, "line {} {:?}", line.n, line.line);
        }
    }
}

#[test]
fn routine_lines_stay_routine_in_phase_b() {
    // A line that runs with no exit in phase A makes no exit in phase B either; phase B
    // only takes exits away, where the snapshot covers what a line destroys, and lets
    // the classifier judge a named persistence change that efrd copies first.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        if matches!(line.pa.as_str(), "R" | "C") {
            assert_eq!(line.pb, line.pa, "line {} {:?}", line.n, line.line);
        }
        if line.pb != line.pa && line.pa == "U(persistence)" {
            assert_eq!(line.pb, "N(persistence)", "line {} {:?}", line.n, line.line);
        } else if line.pb != line.pa {
            assert_eq!(line.pb, "R", "line {} {:?}", line.n, line.line);
            assert_eq!(line.pa, "K(destructive)", "line {} {:?}", line.n, line.line);
        }
    }
}

#[test]
fn routine_lines_stay_routine_in_phase_d1() {
    // A line that runs with no exit in phase B makes no exit in phase D1 either; phase D1
    // only takes exits away: `df` and `findmnt` read the mount view in the sandbox, and a
    // line that only reads the host's network gets the network view.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        if matches!(line.pb.as_str(), "R" | "C") {
            assert_eq!(pd(line), line.pb, "line {} {:?}", line.n, line.line);
        }
        if let Some(pd) = &line.pd {
            assert_ne!(pd, &line.pb, "line {}: a `pd` that equals `pb` says nothing", line.n);
            assert_eq!(pd, "R", "line {} {:?}", line.n, line.line);
            assert_eq!(line.pb, "K(outside)", "line {} {:?}", line.n, line.line);
        }
    }
}

#[test]
fn background_flag_does_not_change_prediction() {
    // A line run as a background job (phase C) has the exits of the same line in the
    // foreground. Only a delete that the snapshot covers differs: a job answers at once,
    // so undo of its turn needs `--force`, and the delete stays an exit.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let engine = undo_engine();
    let decide = |requirements: Requirements| {
        engine.decide(&DecisionInput {
            requirements,
            scope: Scope::Project(id(1)),
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(SCRATCH),
        })
    };
    let kinds =
        |decision: &Decision| -> Vec<ExitKind> { decision.exits().map(|need| need.kind).collect() };
    let mut changed = Vec::new();
    for line in corpus.line.iter().filter(|line| line.line.is_some()) {
        let foreground = decide(requirements(line, true));
        let background = decide(requirements(line, true).with_background());
        if kinds(&foreground) != kinds(&background) {
            assert_eq!(line.pb, "R", "line {}", line.n);
            assert_eq!(kinds(&background), [ExitKind::Destructive], "line {}", line.n);
            changed.push(line.n);
        }
    }
    assert_eq!(changed, [54, 57, 59]);
}

#[test]
fn full_corpus_rows_match() {
    assert_eq!(full_mismatches(&full_engine()), Vec::<String>::new());
    // Full mode needs no sandbox support: the phase A machine with undo decides the same.
    let bare = machine(AutoSupport { undo: true, floor_backup: true, ..AutoSupport::default() });
    assert_eq!(full_mismatches(&bare), Vec::<String>::new());
}

#[test]
fn full_never_judges_by_the_classifier() {
    // Full mode has no reviewer: a line runs, asks the user, or is refused.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        let full = full(line);
        assert!(
            !full.starts_with('K') && !full.starts_with('N') && !full.starts_with('C'),
            "line {}: {full}",
            line.n
        );
    }
}

#[test]
fn full_floor_rows_equal_auto_floor_rows() {
    // A drift guard for the floors that full mode and `auto` share.
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    for line in &corpus.line {
        for auto in [pd(line), pe(line)] {
            assert_eq!(auto == "D", full(line) == "D", "line {} {:?}", line.n, line.line);
        }
    }
}

#[test]
fn full_column_only_where_it_differs_from_the_default() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let mut explicit = BTreeSet::new();
    for line in &corpus.line {
        if let Some(full) = &line.full {
            assert_ne!(
                *full,
                full_default(line),
                "line {}: a `full` that equals the default",
                line.n
            );
            explicit.insert(line.n);
        }
    }
    // The deletes that full mode guards: `..` (it holds three projects), a project
    // root, `~/.config` (it holds efr's config) and `~`; and a read of everything below
    // `~`, which holds secrets that no sandbox masks in full mode.
    assert_eq!(explicit, BTreeSet::from([16, 49, 53, 60, 95, 106]));
}

#[test]
fn auto_is_unchanged_by_full() {
    // Only a full call settles: every line of `auto` keeps its phase E result, with no
    // reason of full mode and no guarded delete.
    assert_eq!(mismatches(&pe_engine(), true, pe), Vec::<String>::new());
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let engine = pe_engine();
    for line in &corpus.line {
        let decision = engine.decide(&input(line));
        assert_eq!(decision.critical().count(), 0, "line {}", line.n);
        assert!(
            !decision.reasons().iter().any(|reason| reason.cause == Cause::Full),
            "line {}",
            line.n
        );
    }
}
