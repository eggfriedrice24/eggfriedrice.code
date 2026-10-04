use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{FIXTURES_DIR, all, event_path, event_samples, method_samples, method_stem};
use crate::{Event, Method};

const BLESS: &str = "cargo test -p efr-protocol --lib -- --ignored --exact \
                     fixtures_check::tests::bless_fixtures";

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES_DIR).join(path)).unwrap_or_else(|err| {
        panic!("cannot read fixtures/v1/{path}: {err}; to create it run {BLESS}")
    })
}

/// Every file under `dir`, as paths relative to `root` with `/` separators.
fn files_under(root: &Path, dir: &Path, found: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(root, &path, found);
        } else {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<_> = relative.iter().map(|part| part.to_str().unwrap()).collect();
            found.insert(parts.join("/"));
        }
    }
}

/// The position of a method in the enum. Adding a method without an arm here does not
/// compile, and an arm without a sample fails `every_method_has_a_params_sample`.
fn method_index(method: &Method) -> usize {
    match method {
        Method::Hello(_) => 0,
        Method::ConversationsList(_) => 1,
        Method::ConversationSubscribe(_) => 2,
        Method::ConversationHistory(_) => 3,
        Method::PromptSend(_) => 4,
        Method::TurnInterrupt(_) => 5,
        Method::TurnSteer(_) => 6,
        Method::ApprovalRespond(_) => 7,
        Method::PtyAttach(_) => 8,
        Method::PtyWrite(_) => 9,
        Method::PtyResize(_) => 10,
        Method::LeaseReport(_) => 11,
        Method::AdminStatus(_) => 12,
        Method::AdminLoginOpenAi(_) => 13,
    }
}

const METHOD_COUNT: usize = 14;

/// The position of an event kind in the enum, for the same purpose as `method_index`.
fn event_index(event: &Event) -> usize {
    match event {
        Event::ConversationCreated { .. } => 0,
        Event::PromptQueued { .. } => 1,
        Event::PromptHeld { .. } => 2,
        Event::TurnStarted { .. } => 3,
        Event::ScopeChanged { .. } => 4,
        Event::AssistantMessageUpdated { .. } => 5,
        Event::AssistantMessageCompleted { .. } => 6,
        Event::ToolCallStarted { .. } => 7,
        Event::ToolCallOutputUpdated { .. } => 8,
        Event::ToolCallCompleted { .. } => 9,
        Event::ApprovalRequested { .. } => 10,
        Event::ApprovalResolved { .. } => 11,
        Event::ApprovalExpired { .. } => 12,
        Event::TurnSteered { .. } => 13,
        Event::TurnInterruptRequested { .. } => 14,
        Event::TurnInterrupted { .. } => 15,
        Event::TurnCompleted { .. } => 16,
        Event::TurnFailed { .. } => 17,
        Event::TurnCancelled { .. } => 18,
        Event::ShellStarted { .. } => 19,
        Event::ShellExited { .. } => 20,
        Event::CwdChanged { .. } => 21,
        Event::LoginCompleted { .. } => 22,
        Event::Unknown { .. } => 23,
    }
}

const EVENT_COUNT: usize = 24;

#[test]
fn every_sample_matches_its_frozen_file() {
    for fixture in all() {
        assert_eq!(
            read(&fixture.path),
            fixture.json,
            "fixtures/v1/{} differs from its sample; if the change is deliberate, run {BLESS} \
             and add a changelog line to docs/protocol.md",
            fixture.path
        );
    }
}

#[test]
fn every_frozen_file_round_trips_byte_for_byte() {
    for fixture in all() {
        let text = read(&fixture.path);
        let again = (fixture.round_trip)(&text)
            .unwrap_or_else(|err| panic!("fixtures/v1/{} does not decode: {err}", fixture.path));
        assert_eq!(again, text, "fixtures/v1/{} changes in a round trip", fixture.path);
    }
}

#[test]
fn every_file_has_a_sample_and_every_sample_has_a_file() {
    let root = Path::new(FIXTURES_DIR);
    let mut on_disk = BTreeSet::new();
    files_under(root, root, &mut on_disk);
    let samples: BTreeSet<String> = all().into_iter().map(|fixture| fixture.path).collect();
    assert_eq!(on_disk, samples);
}

#[test]
fn fixture_paths_are_unique() {
    let paths: Vec<String> = all().into_iter().map(|fixture| fixture.path).collect();
    let unique: BTreeSet<&String> = paths.iter().collect();
    assert_eq!(unique.len(), paths.len());
}

#[test]
fn every_method_has_a_params_sample() {
    let covered: BTreeSet<usize> = method_samples().iter().map(method_index).collect();
    assert_eq!(covered, (0..METHOD_COUNT).collect());
}

#[test]
fn every_method_has_a_result_or_item_fixture() {
    let paths: BTreeSet<String> = all().into_iter().map(|fixture| fixture.path).collect();
    for method in method_samples() {
        let stem = method_stem(&method);
        if method.is_stream() {
            let prefix = format!("{stem}_item_");
            assert!(
                paths.iter().any(|path| path.starts_with(&prefix)),
                "{stem} has no item fixture"
            );
            assert!(!paths.contains(&format!("{stem}_result.json")), "{stem} streams");
        } else {
            assert!(paths.contains(&format!("{stem}_result.json")), "{stem} has no result fixture");
        }
    }
}

#[test]
fn params_files_decode_as_their_method() {
    for method in method_samples() {
        let path = format!("{}_params.json", method_stem(&method));
        let decoded: Method = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(decoded.name(), method.name(), "{path}");
    }
}

#[test]
fn every_event_kind_has_a_sample() {
    let covered: BTreeSet<usize> = event_samples().iter().map(event_index).collect();
    assert_eq!(covered, (0..EVENT_COUNT).collect());
}

#[test]
fn event_files_decode_as_the_kind_they_are_named_after() {
    for sample in event_samples() {
        let path = event_path(&sample);
        let decoded: Event = serde_json::from_str(&read(&path)).unwrap();
        let is_unknown = matches!(decoded, Event::Unknown { .. });
        assert_eq!(is_unknown, matches!(sample, Event::Unknown { .. }), "{path}");
        if !is_unknown {
            assert_eq!(format!("events/{}.json", decoded.kind()), path);
        }
    }
}

/// Rewrites every fixture from its sample. Ignored, so it runs only when asked for.
#[test]
#[ignore = "rewrites fixtures/v1; run it on purpose (see the README)"]
fn bless_fixtures() {
    for fixture in all() {
        let path = Path::new(FIXTURES_DIR).join(&fixture.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, fixture.json).unwrap();
    }
}
