//! A delete or a move of a file: it asks in every mode, from every origin and whatever
//! the rules say, except in `auto` when `efr undo` can bring back each file that it
//! writes (phase B), and its paths are still judged as writes.

use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRATCH, app, input, locations};
use crate::exits::tool_fact_requests;
use crate::{
    Action, AutoSupport, CallFacts, Cause, ConversationPolicy, CoverAsk, Coverage, Decision,
    DecisionInput, Effect, Engine, Policy, Requirements, Resource, Rule, Subject,
};

/// A move of a file in the project `~/p/app`, as `apply_patch` declares it.
fn moved() -> Requirements {
    Requirements::none()
        .with_write("/home/u/p/app/src/old.rs")
        .with_write("/home/u/p/app/src/new.rs")
        .with_destructive()
}

fn allow_everything() -> Policy {
    Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap()
}

fn decide(
    engine: &Engine,
    requirements: Requirements,
    mode: Mode,
    origin: Origin,
    conversation: Policy,
) -> Decision {
    let input = DecisionInput {
        mode,
        conversation_policy: ConversationPolicy::new(SCRATCH).with_rules(conversation),
        ..input(requirements, Scope::Project(app()), origin)
    };
    engine.decide(&input)
}

#[rstest]
#[case::shell(Origin::Shell)]
#[case::cli(Origin::Cli)]
#[case::proxy(Origin::Proxy)]
#[case::phone(Origin::Phone)]
fn a_delete_or_a_move_asks_in_every_mode_also_when_every_rule_allows(#[case] origin: Origin) {
    let engines =
        [Engine::with_defaults(locations()), Engine::with_rules(locations(), allow_everything())];
    for engine in &engines {
        for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
            for conversation in [Policy::empty(), allow_everything()] {
                let decision = decide(engine, moved(), mode, origin, conversation);
                assert_eq!(decision.effect(), Effect::Ask, "{mode} from {origin:?}");
                let reason = decision
                    .reasons()
                    .iter()
                    .find(|reason| reason.subject == Subject::Destructive)
                    .unwrap();
                assert_eq!(
                    (reason.effect, &reason.cause),
                    (Effect::Ask, &Cause::Destructive),
                    "{mode} from {origin:?}"
                );
            }
        }
    }
}

#[test]
fn the_same_paths_without_the_flag_are_routine_in_the_turn_s_project() {
    let engine = Engine::with_defaults(locations());
    let routine = Requirements::none()
        .with_write("/home/u/p/app/src/old.rs")
        .with_write("/home/u/p/app/src/new.rs");
    for mode in [Mode::Cautious, Mode::Auto] {
        let decision = decide(&engine, routine.clone(), mode, Origin::Shell, Policy::empty());
        assert_eq!(decision.effect(), Effect::Allow, "{mode}");
    }
}

#[test]
fn a_denied_path_stays_denied_for_a_delete() {
    let engine = Engine::with_defaults(locations());
    let key = Requirements::none().with_write("/home/u/.ssh/id_ed25519").with_destructive();
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let decision = decide(&engine, key.clone(), mode, Origin::Shell, Policy::empty());
        assert_eq!(decision.effect(), Effect::Deny, "{mode}");
    }
}

#[test]
fn the_reason_says_why_a_person_approves_it() {
    let engine = Engine::with_defaults(locations());
    let decision = decide(&engine, moved(), Mode::Cautious, Origin::Shell, Policy::empty());
    let deciding: Vec<String> = decision.deciding().map(ToString::to_string).collect();
    assert_eq!(
        deciding,
        ["delete or move files: ask, because the user approves each delete and move that efr \
          undo cannot bring back in the auto mode"]
    );
}

#[test]
fn a_delete_or_a_move_is_a_requirement() {
    assert!(!Requirements::none().with_destructive().is_empty());
}

/// The engine of `locations` on a machine with undo and the home store (phase B).
fn undo_engine() -> Engine {
    Engine::with_defaults(locations()).with_support(AutoSupport {
        undo: true,
        floor_backup: true,
        ..AutoSupport::default()
    })
}

/// `requirements` with the facts of a daemon that answers each ask of its file tool
/// request: the asks of `covered` paths with [`Coverage::Covered`], the rest with
/// [`Coverage::LeftOut`].
fn answered(requirements: Requirements, covered: &[&str]) -> Requirements {
    let request = tool_fact_requests(&requirements, &locations());
    let answers = request
        .covers
        .iter()
        .map(|ask| {
            let path = match ask {
                CoverAsk::Path(path) | CoverAsk::Home(path) => path.as_path(),
                _ => Path::new(""),
            };
            let answer = if covered.iter().any(|known| Path::new(known) == path) {
                Coverage::Covered
            } else {
                Coverage::LeftOut
            };
            (ask.clone(), answer)
        })
        .collect();
    requirements.with_facts(CallFacts { covered: answers, ..CallFacts::default() })
}

fn asks_destructive(decision: &Decision) -> bool {
    decision.reasons().iter().any(|reason| reason.cause == Cause::Destructive)
}

fn in_auto(engine: &Engine, requirements: Requirements) -> Decision {
    decide(engine, requirements, Mode::Auto, Origin::Shell, Policy::empty())
}

const OLD: &str = "/home/u/p/app/src/old.rs";
const NEW: &str = "/home/u/p/app/src/new.rs";

#[test]
fn apply_patch_delete_routine_when_covered_in_auto() {
    let engine = undo_engine();
    let deleted = Requirements::none().with_write(OLD).with_destructive();
    for requirements in [answered(deleted, &[OLD]), answered(moved(), &[OLD, NEW])] {
        let decision = in_auto(&engine, requirements.clone());
        assert!(!asks_destructive(&decision), "{requirements:?}");
        assert_eq!(decision.effect(), Effect::Allow, "{requirements:?}");
    }
    // Without undo on the machine, or with no facts, it asks.
    let engine = Engine::with_defaults(locations())
        .with_support(AutoSupport { floor_backup: true, ..AutoSupport::default() });
    assert!(asks_destructive(&in_auto(&engine, answered(moved(), &[OLD, NEW]))));
    assert!(asks_destructive(&in_auto(&undo_engine(), moved())));
}

#[test]
fn apply_patch_move_onto_existing_file_asks_coverage() {
    let engine = undo_engine();
    // The destination of the move is in the request, and an answer that does not cover
    // it keeps the question: the move would replace a file that undo cannot restore.
    let request = tool_fact_requests(&moved(), &locations());
    for path in [OLD, NEW] {
        assert!(request.covers.contains(&CoverAsk::Path(PathBuf::from(path))), "{path}");
    }
    assert!(asks_destructive(&in_auto(&engine, answered(moved(), &[OLD]))));
    // A move out of the project needs the home store's copy of the destination.
    let out = "/home/u/Documents/old.rs";
    let away = || Requirements::none().with_write(OLD).with_write(out).with_destructive();
    let request = tool_fact_requests(&away(), &locations());
    assert!(request.covers.contains(&CoverAsk::Home(PathBuf::from(out))));
    assert!(asks_destructive(&in_auto(&engine, answered(away(), &[OLD]))));
    let decision = in_auto(&engine, answered(away(), &[OLD, out]));
    assert!(!asks_destructive(&decision));
    // The write out of the project is still a `write` exit for the classifier.
    assert!(decision.exits().any(|need| need.kind == ExitKind::Write));
}

#[rstest]
#[case::cautious(Mode::Cautious)]
#[case::manual(Mode::Manual)]
fn apply_patch_delete_asks_in_cautious_and_manual(#[case] mode: Mode) {
    let requirements = answered(moved(), &[OLD, NEW]);
    let decision = decide(&undo_engine(), requirements, mode, Origin::Shell, Policy::empty());
    assert!(asks_destructive(&decision), "{mode}");
    assert_eq!(decision.effect(), Effect::Ask, "{mode}");
}

#[test]
fn apply_patch_delete_from_the_phone_asks() {
    // A remote turn runs as `cautious`, so coverage never lifts its question.
    let requirements = answered(moved(), &[OLD, NEW]);
    let decision = decide(&undo_engine(), requirements, Mode::Auto, Origin::Phone, Policy::empty());
    assert!(asks_destructive(&decision));
}
