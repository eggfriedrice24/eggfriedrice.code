//! Phase B: a destructive part of a line that the turn's snapshot covers runs with no
//! exit (efr's auto spec 10.7), and each home write names what efrd copies first
//! (10.8). The engine only reads the daemon's answers in `CallFacts::covered`.

use std::path::PathBuf;

use efr_protocol::{ExitKind, Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::{app, input};
use crate::exits::{fact_requests, tool_fact_requests};
use crate::{
    Approver, AutoSupport, CallFacts, CoverAsk, Coverage, Decision, DecisionInput, Effect, Engine,
    Requirements, TargetKind,
};

const APP: &str = "/home/u/p/app";

/// A machine with snapshots, undo and the home store.
fn undo_engine() -> Engine {
    Engine::with_defaults(auto_locations()).with_support(AutoSupport {
        undo: true,
        floor_backup: true,
        ..AutoSupport::default()
    })
}

fn in_auto(requirements: Requirements) -> DecisionInput {
    DecisionInput { mode: Mode::Auto, ..input(requirements, Scope::Project(app()), Origin::Shell) }
}

fn decide(engine: &Engine, requirements: Requirements) -> Decision {
    engine.decide(&in_auto(requirements))
}

fn kinds(decision: &Decision) -> Vec<ExitKind> {
    decision.exits().map(|need| need.kind).collect()
}

/// A shell call of `line` in the project root, with the facts that a daemon gives when
/// it answers every ask of the line with `answer`. `targets` and `tracked` are the
/// other facts of the line.
fn shell_with(
    line: &str,
    answer: Coverage,
    targets: &[(&str, Option<TargetKind>)],
    tracked: &[(&str, u32)],
) -> Requirements {
    let request = fact_requests(line, Some(APP.as_ref()), &auto_locations());
    let facts = CallFacts {
        targets: targets.iter().map(|(path, kind)| (PathBuf::from(path), *kind)).collect(),
        tracked_counts: tracked.iter().map(|(path, count)| (PathBuf::from(path), *count)).collect(),
        covered: request.covers.iter().map(|ask| (ask.clone(), answer)).collect(),
        ..CallFacts::default()
    };
    Requirements::none().with_command(line).with_command_dir(APP).with_facts(facts)
}

/// The destructive lines of the corpus that a snapshot can cover, each with the facts
/// that make it destructive.
fn coverable() -> Vec<Requirements> {
    let log = "/home/u/p/app/important.log";
    vec![
        shell_with("git reset --hard HEAD~5", Coverage::Covered, &[], &[]),
        shell_with("git checkout -- src/main.rs", Coverage::Covered, &[], &[]),
        shell_with("git restore src/main.rs", Coverage::Covered, &[], &[]),
        shell_with("git switch --discard-changes main", Coverage::Covered, &[], &[]),
        shell_with("git branch -D old", Coverage::Covered, &[], &[]),
        shell_with("git stash drop", Coverage::Covered, &[], &[]),
        shell_with("git clean -fd", Coverage::Covered, &[], &[]),
        shell_with("dd if=/dev/zero of=db.sqlite", Coverage::Covered, &[], &[]),
        shell_with("truncate -s 0 log.txt", Coverage::Covered, &[], &[]),
        shell_with(": > important.log", Coverage::Covered, &[(log, Some(TargetKind::File))], &[]),
        shell_with("find build -name '*.o' -delete", Coverage::Covered, &[], &[]),
        shell_with("rm -rf src/old", Coverage::Covered, &[], &[("/home/u/p/app/src/old", 4)]),
    ]
}

#[test]
fn destructive_line_routine_when_covered() {
    let engine = undo_engine();
    for requirements in coverable() {
        let decision = decide(&engine, requirements.clone());
        assert_eq!(decision.effect(), Effect::Contain, "{:?}", requirements.command);
        assert_eq!(kinds(&decision), [], "{:?}", requirements.command);
    }
}

#[rstest]
#[case::left_out(Coverage::LeftOut)]
#[case::no_snapshot(Coverage::NoSnapshot)]
#[case::no_mirror(Coverage::NoMirror)]
#[case::not_allowed(Coverage::NotAllowed)]
#[case::unknown(Coverage::Unknown)]
fn destructive_line_stays_with_reviewer_when_not_covered(#[case] answer: Coverage) {
    let engine = undo_engine();
    for covered in coverable() {
        let mut requirements = covered.clone();
        let facts = requirements.facts.as_mut().unwrap();
        for (_, coverage) in &mut facts.covered {
            *coverage = answer;
        }
        let decision = decide(&engine, requirements);
        let need = decision.exits().find(|need| need.kind == ExitKind::Destructive);
        let need = need.unwrap_or_else(|| panic!("{:?}: {:?}", covered.command, kinds(&decision)));
        assert_eq!(need.approver, Approver::Classifier, "{:?}", covered.command);
    }
    // One ask that is not covered keeps the exit: the refs of a reset have no mirror.
    let mut reset = shell_with("git reset --hard HEAD~5", Coverage::Covered, &[], &[]);
    let facts = reset.facts.as_mut().unwrap();
    for (ask, coverage) in &mut facts.covered {
        if matches!(ask, CoverAsk::Refs(_)) {
            *coverage = Coverage::NoMirror;
        }
    }
    assert_eq!(kinds(&decide(&engine, reset)), [ExitKind::Destructive]);
    // No facts at all.
    let bare = Requirements::none().with_command("git reset --hard").with_command_dir(APP);
    assert_eq!(kinds(&decide(&engine, bare)), [ExitKind::Destructive]);
}

#[test]
fn ask_destructive_always_keeps_the_destructive_exit() {
    // `sandbox.ask_destructive = "always"`, or no undo: the machine has no `undo`.
    let engine = Engine::with_defaults(auto_locations())
        .with_support(AutoSupport { floor_backup: true, ..AutoSupport::default() });
    for requirements in coverable() {
        let decision = decide(&engine, requirements.clone());
        assert!(kinds(&decision).contains(&ExitKind::Destructive), "{:?}", requirements.command);
    }
}

#[test]
fn destructive_coverage_keeps_other_exits_of_the_line() {
    let engine = undo_engine();
    let line = "git reset --hard HEAD~5 && git push --force origin main";
    let decision = decide(&engine, shell_with(line, Coverage::Covered, &[], &[]).with_network());
    assert_eq!(kinds(&decision), [ExitKind::Upload]);
    // A covered part goes; a part of the same line outside the roots keeps its exit.
    let line = ": > important.log; truncate -s 0 ~/.bash_history";
    let facts = [("/home/u/p/app/important.log", Some(TargetKind::File))];
    let requirements =
        shell_with(line, Coverage::Covered, &facts, &[]).with_write("/home/u/.bash_history");
    let decision = decide(&engine, requirements);
    let destructive = decision.exits().find(|need| need.kind == ExitKind::Destructive).unwrap();
    assert_eq!(destructive.part, "truncate -s 0 ~/.bash_history");
    assert_eq!(destructive.approver, Approver::User);
    assert!(kinds(&decision).contains(&ExitKind::Write), "{:?}", kinds(&decision));
}

/// `line` with every ask of the request and the extra `asks` answered as covered.
fn claimed_covered(line: &str, asks: &[CoverAsk]) -> Requirements {
    let mut requirements = shell_with(line, Coverage::Covered, &[], &[]);
    let facts = requirements.facts.as_mut().unwrap();
    facts.covered.extend(asks.iter().map(|ask| (ask.clone(), Coverage::Covered)));
    requirements
}

fn in_app(path: &str) -> PathBuf {
    PathBuf::from(APP).join(path)
}

#[test]
fn git_clean_x_is_never_covered() {
    let engine = undo_engine();
    for line in ["git clean -fdx", "git clean -f -X", "git clean -xf"] {
        let request = fact_requests(line, Some(APP.as_ref()), &auto_locations());
        assert!(request.covers.is_empty(), "{line:?} asks nothing: {:?}", request.covers);
        let requirements = claimed_covered(line, &[CoverAsk::Untracked(in_app(""))]);
        assert_eq!(kinds(&decide(&engine, requirements)), [ExitKind::Destructive], "{line:?}");
    }
}

#[test]
fn reflog_expire_is_never_covered() {
    let engine = undo_engine();
    let asks = [CoverAsk::Refs(in_app("")), CoverAsk::Tracked(in_app(""))];
    for line in ["git reflog expire --expire=now --all", "git gc --prune=now"] {
        let requirements = claimed_covered(line, &asks);
        assert_eq!(kinds(&decide(&engine, requirements)), [ExitKind::Destructive], "{line:?}");
    }
}

#[test]
fn shred_is_never_covered() {
    // The user asked to destroy the bytes, so no copy may keep them (corpus 58).
    let engine = undo_engine();
    let line = "shred -u secrets.txt";
    let request = fact_requests(line, Some(APP.as_ref()), &auto_locations());
    assert!(!request.covers.iter().any(|ask| matches!(ask, CoverAsk::Path(_))));
    let requirements = claimed_covered(line, &[CoverAsk::Path(in_app("secrets.txt"))]);
    let decision = decide(&engine, requirements);
    assert_eq!(kinds(&decision), [ExitKind::Destructive]);
    assert_eq!(decision.exits().next().unwrap().approver, Approver::Classifier);
}

#[rstest]
// `~/p/lib` is a `sandbox.write_roots` entry: efrd keeps no snapshot of it.
#[case::user_write_root("dd if=/dev/zero of=~/p/lib/db.sqlite", "/home/u/p/lib/db.sqlite")]
#[case::git_dir("truncate -s 0 .git/index", "/home/u/p/app/.git/index")]
#[case::agent_config("truncate -s 0 .mcp.json", "/home/u/p/app/.mcp.json")]
#[case::outside("truncate -s 0 ~/notes.txt", "/home/u/notes.txt")]
#[case::private_tmp("truncate -s 0 /tmp/x.log", "/tmp/x.log")]
fn destructive_in_user_write_root_is_never_covered(#[case] line: &str, #[case] target: &str) {
    let engine = undo_engine();
    let requirements = claimed_covered(line, &[CoverAsk::Path(PathBuf::from(target))]);
    let decision = decide(&engine, requirements);
    assert!(kinds(&decision).contains(&ExitKind::Destructive), "{line:?}: {:?}", kinds(&decision));
}

#[rstest]
// Corpus 53 and 60: a delete of a whole root, or of a dir above one.
#[case::project_root("rm -rf ~/p/app", "/home/u/p/app")]
#[case::config("rm -rf ~/.config", "/home/u/.config")]
#[case::parent("rm -rf ..", "/home/u/p")]
fn destructive_above_root_never_routine(#[case] line: &str, #[case] target: &str) {
    let engine = undo_engine();
    let asks = [CoverAsk::Path(PathBuf::from(target)), CoverAsk::HomeDir(PathBuf::from(target))];
    let requirements = claimed_covered(line, &asks).with_write(target);
    let decision = decide(&engine, requirements);
    let kinds = kinds(&decision);
    assert!(kinds.contains(&ExitKind::AboveRoot), "{line:?}: {kinds:?}");
    assert!(kinds.contains(&ExitKind::Destructive), "{line:?}: {kinds:?}");
    assert_eq!(decision.effect(), Effect::Ask, "{line:?}");
}

#[test]
fn the_daemon_is_asked_what_the_engine_reads() {
    // Every ask that a decision reads comes from the request, so a daemon that answers
    // the request answers the decision.
    for requirements in coverable() {
        let line = requirements.command.as_deref().unwrap();
        let request = fact_requests(line, Some(APP.as_ref()), &auto_locations());
        assert!(!request.covers.is_empty(), "{line:?}");
    }
    let request = fact_requests("git -C sub reset --hard", Some(APP.as_ref()), &auto_locations());
    assert!(request.covers.contains(&CoverAsk::Tracked(in_app("sub"))));
    // A write of the line asks for the home store, and so does a declared write.
    let request = fact_requests("echo x > ~/notes.txt", Some(APP.as_ref()), &auto_locations())
        .with_writes(&[PathBuf::from("/home/u/Documents/a.txt")], &auto_locations());
    for ask in [
        CoverAsk::Home(PathBuf::from("/home/u/notes.txt")),
        CoverAsk::Home(PathBuf::from("/home/u/Documents/a.txt")),
        CoverAsk::HomeDir(PathBuf::from("/home/u/Documents")),
    ] {
        assert!(request.covers.contains(&ask), "{ask:?}: {:?}", request.covers);
    }
}

/// The exit of `kind` of `requirements` on `engine`.
fn exit_of(engine: &Engine, requirements: Requirements, kind: ExitKind) -> crate::ExitNeed {
    let decision = decide(engine, requirements);
    let need = decision.exits().find(|need| need.kind == kind).cloned();
    need.unwrap_or_else(|| panic!("no {kind:?} exit: {:?}", kinds(&decision)))
}

/// Facts with the kinds of `targets` and every ask of the request of `line` and of the
/// declared `writes` answered with `answer`.
fn home_facts(
    line: Option<&str>,
    writes: &[&str],
    targets: &[(&str, Option<TargetKind>)],
    answer: Coverage,
) -> CallFacts {
    let locations = auto_locations();
    let writes: Vec<PathBuf> = writes.iter().map(PathBuf::from).collect();
    let request = match line {
        Some(line) => fact_requests(line, Some(APP.as_ref()), &locations),
        None => {
            let declared = writes
                .iter()
                .fold(Requirements::none(), |declared, path| declared.with_write(path));
            tool_fact_requests(&declared, &locations)
        }
    }
    .with_writes(&writes, &locations);
    CallFacts {
        targets: targets.iter().map(|(path, kind)| (PathBuf::from(path), *kind)).collect(),
        covered: request.covers.iter().map(|ask| (ask.clone(), answer)).collect(),
        ..CallFacts::default()
    }
}

#[test]
fn home_write_need_copies_before_write() {
    let engine = undo_engine();
    let file = Some(TargetKind::File);
    let dir = Some(TargetKind::Dir);
    // A new file that the line makes.
    let notes = "/home/u/notes.txt";
    let line = "echo x > ~/notes.txt";
    let facts =
        home_facts(Some(line), &[notes], &[(notes, None), ("/home/u", dir)], Coverage::Covered);
    let shell = |line: &str| Requirements::none().with_command(line).with_command_dir(APP);
    let need = exit_of(&engine, shell(line).with_write(notes).with_facts(facts), ExitKind::Write);
    assert_eq!(need.copies, [CoverAsk::Home(PathBuf::from(notes))]);
    assert!(need.backup_before_write);
    assert_eq!(need.covered, Some(true));
    // A dir target: the dir with what it holds.
    let docs = "/home/u/Documents";
    let line = "cp report.pdf ~/Documents/";
    let facts = home_facts(Some(line), &[docs], &[(docs, dir)], Coverage::Covered);
    let need = exit_of(&engine, shell(line).with_write(docs).with_facts(facts), ExitKind::Write);
    assert_eq!(need.copies, [CoverAsk::HomeDir(PathBuf::from(docs))]);
    assert_eq!(need.covered, Some(true));
    // An existing file whose parent the call gets whole: the file and the parent.
    let report = "/home/u/Documents/report.txt";
    let line = "mv x ~/Documents/report.txt";
    let facts =
        home_facts(Some(line), &[report], &[(report, file), (docs, dir)], Coverage::OverLimit);
    let need = exit_of(&engine, shell(line).with_write(report).with_facts(facts), ExitKind::Write);
    assert_eq!(need.bind, Some(crate::WriteBind::Parent(PathBuf::from(docs))));
    assert_eq!(
        need.copies,
        [CoverAsk::Home(PathBuf::from(report)), CoverAsk::HomeDir(PathBuf::from(docs))]
    );
    // Over the limits of the home store: still a write for the classifier, not covered.
    assert_eq!(need.covered, Some(false));
    assert_eq!(need.approver, Approver::Classifier);
    // `write_file` outside the roots.
    let facts = home_facts(None, &[report], &[(report, file)], Coverage::Covered);
    let tool = Requirements::none().with_write(report).with_facts(facts);
    let need = exit_of(&engine, tool, ExitKind::Write);
    assert_eq!(need.copies, [CoverAsk::Home(PathBuf::from(report))]);
    assert_eq!(need.covered, Some(true));
    // With no fact, the copy is not covered.
    let need = exit_of(&engine, Requirements::none().with_write(report), ExitKind::Write);
    assert_eq!(need.covered, Some(false));
    // A cache delete copies the cache file.
    let cache = PathBuf::from("/home/u/.cache/x/data");
    let decision = engine.decide_cache_delete(
        std::slice::from_ref(&cache),
        &Scope::Project(app()),
        super::SCRATCH.as_ref(),
    );
    let need = decision.exits().next().unwrap();
    assert_eq!(need.copies, [CoverAsk::Home(cache)]);
    assert!(need.backup_before_write);
    // Without the home store efrd copies nothing.
    let engine = Engine::with_defaults(auto_locations());
    let need = exit_of(&engine, Requirements::none().with_write(report), ExitKind::Write);
    assert!(need.copies.is_empty() && !need.backup_before_write && need.covered.is_none());
    // A write inside the turn's project is no exit and copies nothing.
    let decision = decide(&undo_engine(), Requirements::none().with_write(format!("{APP}/a.rs")));
    assert_eq!(kinds(&decision), []);
}

#[test]
fn masked_home_write_is_not_covered_but_runs() {
    // Plan PB14 and the phase A answer to OQ6: a named write to a masked file runs with
    // no copy, and the record says that undo cannot bring it back.
    let engine = undo_engine();
    let config = "/home/u/.aws/config";
    let facts = home_facts(None, &[config], &[(config, Some(TargetKind::File))], Coverage::Masked);
    let tool = Requirements::none().with_write(config).with_facts(facts.clone());
    let decision = decide(&engine, tool);
    assert_eq!(decision.effect(), Effect::Ask);
    let need = decision.exits().find(|need| need.kind == ExitKind::Write).unwrap();
    assert_eq!(need.approver, Approver::Classifier);
    assert_eq!(need.copies, [CoverAsk::Home(PathBuf::from(config))]);
    assert_eq!(need.covered, Some(false));
    // The same through a shell line, which runs in the exit child.
    let line = "echo region = x >> ~/.aws/config";
    let facts =
        home_facts(Some(line), &[config], &[(config, Some(TargetKind::File))], Coverage::Masked);
    let shell = Requirements::none()
        .with_command(line)
        .with_command_dir(APP)
        .with_write(config)
        .with_facts(facts);
    let need = exit_of(&engine, shell, ExitKind::Write);
    assert_eq!(need.bind, Some(crate::WriteBind::ExitChild));
    assert_eq!(need.covered, Some(false));
    assert_eq!(need.approver, Approver::Classifier);
}
