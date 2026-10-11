//! The command and attack corpus through the engine in the `auto` mode: every line
//! gets the phase A result of its `pa` column and, on a machine with undo, the phase B
//! result of its `pb` column (`tests/fixtures/auto-corpus.toml`). The `p1` column keeps
//! the phase 1 result of spec Appendix A.

use std::collections::BTreeSet;
use std::path::PathBuf;

use efr_permissions::exits::fact_requests;
use efr_permissions::{
    Approver, AutoSupport, CallFacts, Cause, ConversationPolicy, CoverAsk, Coverage, Decision,
    DecisionInput, Effect, Engine, Locations, Policy, Requirements, SettingsChange, Source,
    SourceFact, TargetKind,
};
use efr_protocol::{ExitKind, Mode, Origin, ProjectId, Scope};
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
        None if form == "linger" => CoverAsk::Linger,
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
    })
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
