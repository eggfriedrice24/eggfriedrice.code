//! A change of efr's settings: it asks in every mode and whatever the rules say, a
//! remote turn may never make one, and only a rule that names secrets counts as one
//! the settings tool must not write.

use std::path::PathBuf;

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRATCH, app, input, locations};
use crate::{
    Action, Cause, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, PathClass,
    Policy, Requirements, Resource, Rule, SettingsChange, Subject,
};

fn change(loosens: bool) -> Requirements {
    Requirements::none()
        .with_settings_change(SettingsChange::new("set model.name = \"gpt-5.4\"", loosens))
}

fn allow_everything() -> Policy {
    Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap()
}

fn decide(engine: &Engine, mode: Mode, origin: Origin, conversation: Policy) -> (Effect, Cause) {
    let input = DecisionInput {
        mode,
        conversation_policy: ConversationPolicy::new(SCRATCH).with_rules(conversation),
        ..input(change(false), Scope::Project(app()), origin)
    };
    let decision = engine.decide(&input);
    let reason = decision.reasons().last().unwrap().clone();
    (decision.effect(), reason.cause)
}

#[rstest]
#[case::shell(Origin::Shell)]
#[case::cli(Origin::Cli)]
#[case::proxy(Origin::Proxy)]
fn a_settings_change_asks_in_every_mode_also_when_every_rule_allows(#[case] origin: Origin) {
    let engines =
        [Engine::with_defaults(locations()), Engine::with_rules(locations(), allow_everything())];
    for engine in &engines {
        for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
            for conversation in [Policy::empty(), allow_everything()] {
                assert_eq!(
                    decide(engine, mode, origin, conversation),
                    (Effect::Ask, Cause::SettingsChange),
                    "{mode} from {origin:?}"
                );
            }
        }
    }
}

#[test]
fn a_remote_turn_may_never_change_the_settings() {
    let engine = Engine::with_rules(locations(), allow_everything());
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        assert_eq!(
            decide(&engine, mode, Origin::Phone, allow_everything()),
            (Effect::Deny, Cause::RemoteSettings { origin: Origin::Phone }),
            "{mode}"
        );
    }
}

#[test]
fn the_reason_shows_the_change_and_marks_one_that_loosens_permissions() {
    let engine = Engine::with_defaults(locations());
    let decide = |loosens| {
        engine.decide(&input(change(loosens), Scope::Machine, Origin::Shell)).reasons()[0].clone()
    };

    let plain = decide(false);
    let looser = decide(true);

    assert_eq!(
        plain.subject,
        Subject::Settings { summary: "set model.name = \"gpt-5.4\"".to_owned(), loosens: false }
    );
    assert_eq!(
        plain.to_string(),
        "change settings: set model.name = \"gpt-5.4\": ask, because only the user approves a \
         change of efr's settings"
    );
    assert_eq!(
        looser.subject.to_string(),
        "change settings: set model.name = \"gpt-5.4\" (loosens permissions)"
    );
}

#[test]
fn a_settings_change_is_a_requirement() {
    assert!(!change(false).is_empty());
    assert!(Requirements::none().is_empty());
}

#[rstest]
#[case::the_class(Resource::Class(PathClass::Secrets), true)]
#[case::under_ssh(Resource::Under("~/.ssh".into()), true)]
#[case::a_key(Resource::Under("~/.ssh/id_ed25519".into()), true)]
#[case::absolute_ssh(Resource::Under("/home/u/.ssh".into()), true)]
#[case::a_secret_path_of_the_user(Resource::Under("~/.config/rclone/rclone.conf".into()), true)]
#[case::above_a_secret(Resource::Under("~".into()), false)]
#[case::user_config(Resource::Class(PathClass::UserConfig), false)]
#[case::nvim(Resource::Under("~/.config/nvim".into()), false)]
#[case::any(Resource::Any, false)]
#[case::project(Resource::Project, false)]
#[case::command(Resource::Command(CommandPattern::new("cat")), false)]
fn a_rule_names_secrets_by_their_class_or_a_path_at_or_below_one(
    #[case] resource: Resource,
    #[case] names: bool,
) {
    let locations =
        locations().with_secret_root(PathBuf::from("/home/u/.config/rclone/rclone.conf")).unwrap();
    let engine = Engine::with_defaults(locations);

    assert_eq!(engine.names_secrets(&resource), names, "{resource:?}");
}
