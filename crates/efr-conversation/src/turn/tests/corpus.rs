//! The routine lines of the command corpus through a turn in `auto`: none of them asks
//! the classifier (efr's auto spec, section 16.3). The corpus and its machine are those
//! of `efr-permissions/tests/fixtures/auto-corpus.toml`.

use std::path::PathBuf;
use std::sync::Arc;

use efr_permissions::exits::fact_requests;
use efr_permissions::{
    AutoSupport, CallFacts, Engine, Locations, Policy, Requirements, TargetKind,
};
use efr_protocol::{Mode, ProjectId, Scope};
use efr_scope::{Basis, Derivation};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use serde_json::json;

use super::review::count;
use crate::testing::{FakeJudge, Script, Setup, allows};

const CORPUS: &str = include_str!("../../../../efr-permissions/tests/fixtures/auto-corpus.toml");

const HOME: &str = "/home/u";
const PROJECT: &str = "/home/u/p/efr";

#[derive(Debug, Deserialize)]
struct Corpus {
    line: Vec<Line>,
}

/// The parts of a line of the fixture that this test reads; its header says what each
/// key means.
#[derive(Debug, Deserialize)]
struct Line {
    #[serde(default)]
    line: Option<String>,
    pa: String,
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
struct Tracked {
    path: PathBuf,
    count: u32,
}

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
    Engine::with_rules(locations, Policy::empty())
        .with_support(AutoSupport { floor_backup: true, ..AutoSupport::default() })
}

/// What the shell tool declares for `line`, with the facts that the daemon collects.
fn requirements(line: &Line, text: &str) -> Requirements {
    let mut requirements = Requirements::none().with_command(text).with_command_dir(PROJECT);
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
    let request = fact_requests(text, Some(PROJECT.as_ref()), &locations);
    let programs = request
        .programs
        .iter()
        .map(|word| {
            let resolved = (!request.builtins.contains(word))
                .then(|| PathBuf::from("/usr/bin").join(word.rsplit('/').next().unwrap()));
            (word.clone(), resolved, false)
        })
        .collect();
    requirements.with_facts(CallFacts { targets, tracked_counts, programs })
}

#[tokio::test]
async fn routine_corpus_lines_make_no_judge_call() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    let routine: Vec<(String, Requirements)> = corpus
        .line
        .iter()
        .filter(|line| line.pa == "R")
        .filter_map(|line| {
            let text = line.line.clone()?;
            let requirements = requirements(line, &text);
            Some((text, requirements))
        })
        .collect();
    assert!(routine.len() > 50, "the corpus has its routine lines: {}", routine.len());
    // Every call would be allowed, so a judge call shows in the count and in nothing else.
    let judge = FakeJudge::answering(vec![allows(); routine.len()]);
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    setup.config.max_model_calls = 1000;
    let derivation =
        Derivation { scope: Scope::Project(id(1)), basis: Basis::Registered, repo: None };
    setup.scope = std::mem::take(&mut setup.scope).with(setup.cwd.clone(), derivation);
    setup.engine = Some(engine());
    setup.judge = Some(Arc::clone(&judge));
    let calls = routine.iter().map(|(text, _)| ("shell", json!({ "command": text }))).collect();
    let script = Script::new(calls);
    let mut h = setup.start_model(Arc::clone(&script) as Arc<dyn efr_provider::Provider>).await;
    h.toolbox.declared.lock().unwrap().extend(routine.iter().cloned());

    let sent = h.prompt("work on efr").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(judge.calls(), 0);
    assert_eq!(count(&h, "exit_requested").await, 0);
    assert_eq!(count(&h, "approval_requested").await, 0);
    assert_eq!(h.toolbox.invoked().len(), routine.len(), "each line ran in the sandbox");
}
