//! Phase B: a destructive part of a line that the turn's snapshot covers runs with no
//! exit (efr's auto spec 10.7). The engine only reads the daemon's answers in
//! `CallFacts::covered`.

use std::path::PathBuf;

use efr_protocol::{ExitKind, Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::{app, input};
use crate::exits::fact_requests;
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
}
