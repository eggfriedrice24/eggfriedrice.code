//! Who approves an exit once the user's messages are known, and what the record holds
//! for the classifier.

use std::path::{Path, PathBuf};

use efr_permissions::{
    Approver, AutoSupport, CallFacts, Decision, Engine, Locations, Requirements, TargetKind,
};
use efr_protocol::{ActionFacts, ExitKind, ExitRecord, ExitSource, Launch, PathClassName, Scope};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Action, Payload, TurnExits, payload, record, route};
use super::{APP, HOME, SCRATCH, app, decide, shell};

/// The test machine with the backup store, the caches, the build output names and
/// efr's trash.
fn engine() -> Engine {
    let mut locations = Locations::new(HOME)
        .unwrap()
        .with_project(app(), APP)
        .unwrap()
        .with_project(other(), "/home/u/p/other")
        .unwrap();
    for root in ["/home/u/.cache", "/home/u/.cargo"] {
        locations = locations.with_cache_root(root).unwrap();
    }
    let names = ["target", "node_modules", "dist", "build", ".venv"].map(str::to_owned);
    let locations = locations
        .with_trash_root("/home/u/.local/share/efr/trash")
        .unwrap()
        .with_rebuildable_names(names);
    Engine::with_defaults(locations)
        .with_support(AutoSupport { floor_backup: true, ..AutoSupport::default() })
}

fn other() -> efr_protocol::ProjectId {
    "0192f0c1-7a00-7000-8000-000000000002".parse().unwrap()
}

fn messages(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

/// Facts that give `path` the kind `kind`.
fn file_fact(path: &str) -> CallFacts {
    CallFacts { targets: vec![(path.into(), Some(TargetKind::File))], ..CallFacts::default() }
}

/// The record of `decision` about a call of `tool` that declared `requirements`.
fn record_of(
    engine: &Engine,
    tool: &str,
    decision: &Decision,
    requirements: &Requirements,
    payload: Option<&Payload>,
    said: &[String],
) -> ExitRecord {
    let launch = Launch::Direct;
    let action = Action {
        tool,
        decision,
        requirements,
        launch: &launch,
        cwd: Path::new(APP),
        scope: &Scope::Project(app()),
        scratch: Path::new(SCRATCH),
        engine,
        payload,
    };
    record(action, said, &TurnExits::default())
}

#[test]
fn a_named_upload_goes_to_the_classifier_and_an_unnamed_one_to_the_user() {
    let decision = decide(&engine(), shell("git push origin main"));
    assert_eq!(route(&decision, &messages(&["fix the typo"]), None), Some(Approver::User));
    let named = messages(&["fix the typo", "then push this branch"]);
    assert_eq!(route(&decision, &named, None), Some(Approver::Classifier));
    assert_eq!(route(&decide(&engine(), shell("ls")), &named, None), None, "no exit");
}

#[test]
fn the_name_check_reads_every_message_not_the_cut_list() {
    let decision = decide(&engine(), shell("git push origin main"));
    // The record keeps 24 KiB of messages, the newest last; the name is older.
    let mut said = messages(&["push it when the build is green"]);
    said.push("x".repeat(ExitRecord::MAX_USER_MESSAGES_BYTES));
    assert_eq!(route(&decision, &said, None), Some(Approver::Classifier));
    let requirements = shell("git push origin main");
    let record = record_of(&engine(), "shell", &decision, &requirements, None, &said);
    assert_eq!(record.user_messages.len(), 1, "the record keeps the cut list");
}

#[test]
fn floor_backup_fact_in_record() {
    let zshrc = "/home/u/.zshrc";
    let requirements =
        shell("echo 'alias k=kubectl' >> ~/.zshrc").with_write(zshrc).with_facts(file_fact(zshrc));
    let engine = engine();
    let decision = decide(&engine, requirements.clone());
    let said = messages(&["add alias k=kubectl to my zshrc"]);

    let record = record_of(&engine, "shell", &decision, &requirements, None, &said);

    let target = record.facts.targets.iter().find(|target| target.path == Path::new(zshrc));
    let target = target.expect("the zshrc is a target");
    assert!(target.backup_before_write);
    assert!(target.floor);
    assert!(target.named_in_user_messages, "\"my zshrc\" names it by its word form");
    assert_eq!(route(&decision, &said, None), Some(Approver::Classifier));
    let unnamed = messages(&["tidy my dotfiles"]);
    assert_eq!(route(&decision, &unnamed, None), Some(Approver::User));
    let record = record_of(&engine, "shell", &decision, &requirements, None, &unnamed);
    assert!(!record.facts.targets.iter().any(|target| target.named_in_user_messages));
}

#[test]
fn a_cache_delete_target_has_the_cache_class() {
    let engine = engine();
    let paths = [PathBuf::from("/home/u/.cache/go-build/ab/cd")];
    let decision = engine.decide_cache_delete(&paths, &Scope::Project(app()), Path::new(SCRATCH));
    let requirements = shell("go clean -cache");

    let record = record_of(&engine, "shell", &decision, &requirements, None, &[]);

    assert_eq!(record.action.source, ExitSource::CacheDelete);
    assert_eq!(record.action.exits, vec![ExitKind::Write]);
    assert_eq!(record.facts.targets[0].class, Some(PathClassName::Cache));
    assert_eq!(route(&decision, &[], None), Some(Approver::Classifier), "no name needed");
}

#[test]
fn classifier_record_has_floor_edit_payload() {
    let zshrc = "/home/u/.zshrc";
    let engine = engine();
    let requirements = Requirements::none().with_write(zshrc).with_facts(file_fact(zshrc));
    let decision = decide(&engine, requirements.clone());
    let home = Path::new(HOME);

    let content = json!({ "path": zshrc, "content": "alias k=kubectl\n" });
    let written = payload("write_file", &content, &decision, home, Path::new(APP)).unwrap();
    assert!(written.judgeable());
    let record = record_of(&engine, "write_file", &decision, &requirements, Some(&written), &[]);
    assert_eq!(record.action.edit.as_deref(), Some("alias k=kubectl\n"));
    assert_eq!(record.action.line, "", "a file tool has no line");

    let input =
        json!({ "path": zshrc, "old_string": "alias l=ls", "new_string": "alias l='ls -la'" });
    let edited = payload("edit", &input, &decision, home, Path::new(APP)).unwrap();
    assert_eq!(edited.recorded(), "old_string:\nalias l=ls\nnew_string:\nalias l='ls -la'");

    let patch = "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n*** Update File: \
                 ~/.zshrc\n@@\n-alias l=ls\n+alias l='ls -la'\n*** End Patch\n";
    let patched = payload("apply_patch", &json!(patch), &decision, home, Path::new(APP)).unwrap();
    assert_eq!(
        patched.recorded(),
        "*** Update File: ~/.zshrc\n@@\n-alias l=ls\n+alias l='ls -la'\n"
    );

    // A call that writes no floor file has no payload; neither has a shell line.
    let plain = Requirements::none().with_write("/home/u/p/app/notes.txt");
    let plain = decide(&engine, plain);
    assert_eq!(payload("write_file", &content, &plain, home, Path::new(APP)), None);
    assert_eq!(payload("shell", &content, &decision, home, Path::new(APP)), None);
}

#[test]
fn a_patch_that_names_a_floor_file_only_by_its_link_sends_the_whole_patch() {
    let real = "/home/u/dotfiles/zsh/.zshrc";
    let engine = engine();
    let decision = decide(
        &engine,
        Requirements::none().with_write("/home/u/.zshrc").with_facts(file_fact("/home/u/.zshrc")),
    );
    let need = decision.exits().next().unwrap();
    assert!(need.backup_before_write, "{need:?}");
    let patch = format!("*** Begin Patch\n*** Update File: {real}\n@@\n-a\n+b\n*** End Patch\n");
    let found = payload("apply_patch", &json!(patch), &decision, Path::new(HOME), Path::new(APP));
    assert_eq!(found.unwrap().recorded(), patch);
}

#[test]
fn floor_edit_payload_over_limit_asks_user() {
    let zshrc = "/home/u/.zshrc";
    let engine = engine();
    let requirements = Requirements::none().with_write(zshrc).with_facts(file_fact(zshrc));
    let decision = decide(&engine, requirements.clone());
    let long = "x".repeat(ActionFacts::MAX_EDIT_BYTES + 1);
    let input = json!({ "path": zshrc, "content": long });
    let found = payload("write_file", &input, &decision, Path::new(HOME), Path::new(APP)).unwrap();
    let said = messages(&["rewrite my zshrc"]);

    assert!(!found.judgeable());
    assert_eq!(route(&decision, &said, Some(&found)), Some(Approver::User));
    let record = record_of(&engine, "write_file", &decision, &requirements, Some(&found), &said);
    let edit = record.action.edit.unwrap();
    assert!(edit.len() <= ActionFacts::MAX_EDIT_BYTES, "{}", edit.len());
    assert!(edit.ends_with("so the user decides]"), "{edit}");
    // A payload at the limit is judged whole.
    let input = json!({ "path": zshrc, "content": "x".repeat(ActionFacts::MAX_EDIT_BYTES) });
    let found = payload("write_file", &input, &decision, Path::new(HOME), Path::new(APP)).unwrap();
    assert_eq!(route(&decision, &said, Some(&found)), Some(Approver::Classifier));
    // An input without its text is no payload that the classifier can judge.
    let found = payload(
        "write_file",
        &json!({ "path": zshrc }),
        &decision,
        Path::new(HOME),
        Path::new(APP),
    );
    assert_eq!(route(&decision, &said, found.as_ref()), Some(Approver::User));
}
