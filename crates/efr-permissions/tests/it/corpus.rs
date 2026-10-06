//! The command and attack corpus through the engine in the `auto` mode: every line
//! gets the phase 1 result of spec Appendix A (`tests/fixtures/auto-corpus.toml`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use efr_permissions::{
    CallFacts, Cause, ConversationPolicy, Decision, DecisionInput, Effect, Engine, Locations,
    Policy, Requirements, SettingsChange, TargetKind,
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
    #[serde(default)]
    note: Option<String>,
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

/// The machine of the fixture's header.
fn engine() -> Engine {
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
    for root in [
        "/tmp",
        "/var/tmp",
        "/dev/shm",
        "/home/u/.cargo",
        "/home/u/.rustup",
        "/home/u/.cache",
        "/home/u/go/pkg/mod",
        "/home/u/.npm",
        "/home/u/p/app",
        "/home/u/p/project-x",
    ] {
        locations = locations.with_envelope_root(root).unwrap();
    }
    Engine::with_rules(locations, Policy::empty())
}

/// What the call of `line` declares, with the facts the daemon would collect.
fn requirements(line: &Line) -> Requirements {
    if let Some(summary) = &line.settings {
        return Requirements::none().with_settings_change(SettingsChange::new(summary, true));
    }
    let mut requirements =
        Requirements::none().with_command(line.line.clone().unwrap()).with_command_dir(PROJECT);
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
    requirements.with_facts(CallFacts { targets, tracked_counts, programs: Vec::new() })
}

/// The result of `decision` in the words of Appendix A: `R` for a line that runs
/// contained with no exit (the corpus's `C` lines too), `X(kind)` and `U(kind)` for an
/// exit that asks, and `D` for a floor.
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
            let letter = if decision.exits().any(|need| need.user_only) { "U" } else { "X" };
            // NOTE: the kind that Appendix A names must be among the exits; the fixture's
            // `exits` holds the full set.
            let named = expected
                .strip_prefix(['X', 'U'])
                .and_then(|rest| rest.strip_prefix('('))
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or_default();
            let kind = decision
                .exits()
                .map(|need| need.kind.as_str())
                .find(|kind| *kind == named)
                .unwrap_or("none of them");
            format!("{letter}({kind})")
        }
    }
}

/// How strict a result is: a floor over a user-only exit over an exit over a routine
/// or contained line.
fn rank(result: &str) -> u8 {
    match result.chars().next() {
        Some('D') => 3,
        Some('U') => 2,
        Some('X') => 1,
        _ => 0,
    }
}

#[test]
fn every_line_has_the_phase1_result() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let engine = engine();
    let mut failures = Vec::new();
    for line in &corpus.line {
        let input = DecisionInput {
            requirements: requirements(line),
            scope: Scope::Project(id(1)),
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(SCRATCH),
        };
        let decision = engine.decide(&input);
        let expected = line.engine.as_deref().unwrap_or(&line.p1);
        let found = verdict(&decision, expected);
        let exits: BTreeSet<ExitKind> = decision.exits().map(|need| need.kind).collect();
        let wanted: BTreeSet<ExitKind> = line.exits.iter().copied().collect();
        let text = line.line.as_deref().or(line.settings.as_deref()).unwrap_or_default();
        if found != expected || exits != wanted {
            failures.push(format!(
                "line {} {text:?}: expected {expected} with {wanted:?}, found {found} with {exits:?}",
                line.n
            ));
        }
    }
    assert_eq!(failures, Vec::<String>::new());
}

#[test]
fn the_fixture_holds_all_110_lines_and_only_stricter_overrides() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let numbers: BTreeSet<u32> = corpus.line.iter().map(|line| line.n).collect();
    assert_eq!(numbers, (1..=110).collect::<BTreeSet<u32>>());
    for line in &corpus.line {
        assert!(line.line.is_some() != line.settings.is_some(), "line {}", line.n);
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
