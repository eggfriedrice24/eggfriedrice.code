//! The shell tool declares for each line of the `auto` corpus what the corpus fixture
//! of `efr-permissions` says it declares, so the engine's corpus test judges what the
//! daemon would really pass it.

use std::path::PathBuf;
use std::sync::Arc;

use efr_scope::Home;
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use serde_json::json;

use super::tool;
use crate::testing::ids;
use crate::{AccessMode, MemoryJournal, Tool as _, ToolContext, WriteJournal};

const CORPUS: &str = include_str!("../../../../efr-permissions/tests/fixtures/auto-corpus.toml");

#[derive(Debug, Deserialize)]
struct Corpus {
    line: Vec<Line>,
}

/// The keys of a line that say what the shell tool declares; the fixture's other keys
/// belong to the engine's test.
#[derive(Debug, Deserialize)]
struct Line {
    n: u32,
    #[serde(default)]
    line: Option<String>,
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
}

#[test]
fn the_shell_tool_declares_what_the_auto_corpus_says() {
    let corpus: Corpus = toml::from_str(CORPUS).unwrap();
    // The machine of the fixture's header: the home directory and the turn's project,
    // where the line runs.
    let context = ToolContext::new(
        ids(),
        "/home/u/p/efr",
        "/home/u/.local/share/efr/scratch/2026-10-06-corpus-0a1b2c3d",
        Home::new("/home/u").unwrap(),
        TestClock::new().shared(),
        Arc::new(MemoryJournal::new()) as Arc<dyn WriteJournal>,
    );
    let mut checked = 0;
    for line in &corpus.line {
        let Some(text) = &line.line else { continue };
        let declared = tool().requirements(&context, &json!({ "command": text })).unwrap();
        let paths = |mode: AccessMode| -> Vec<PathBuf> {
            declared
                .paths
                .iter()
                .filter(|path| path.mode == mode)
                .map(|path| path.path.clone())
                .collect()
        };
        let found = (
            paths(AccessMode::Read),
            paths(AccessMode::ReadTree),
            paths(AccessMode::Write),
            declared.network,
            declared.interactive,
        );
        let fixture = (
            line.reads.clone(),
            line.trees.clone(),
            line.writes.clone(),
            line.network,
            line.interactive,
        );
        assert_eq!(found, fixture, "line {} {text:?}", line.n);
        checked += 1;
    }
    assert!(checked > 110, "{checked}");
}
