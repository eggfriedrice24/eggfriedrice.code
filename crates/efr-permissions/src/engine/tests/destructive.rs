//! A delete or a move of a file: it asks in every mode, from every origin and whatever
//! the rules say, until undo can bring the file back, and its paths are still judged
//! as writes.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRATCH, app, input, locations};
use crate::{
    Action, Cause, ConversationPolicy, Decision, DecisionInput, Effect, Engine, Policy,
    Requirements, Resource, Rule, Subject,
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
        ["delete or move files: ask, because undo cannot bring the file back yet, so the user \
          approves each delete and move"]
    );
}

#[test]
fn a_delete_or_a_move_is_a_requirement() {
    assert!(!Requirements::none().with_destructive().is_empty());
}
