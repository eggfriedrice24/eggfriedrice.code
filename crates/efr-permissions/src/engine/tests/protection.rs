//! Config protection: no tool writes efr's config directory, or the real file behind a
//! symbolic link in it, in any mode and whatever the rules say; reading stays free.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRATCH, app, input, locations};
use crate::{
    Action, Cause, ConversationPolicy, DecisionInput, Effect, Engine, Locations, PathClass, Policy,
    Requirements, Resource, Rule,
};

const CONFIG: &str = "/home/u/.config/efr";
/// The real file behind `~/.config/efr/config.toml`, in a dotfiles repository.
const DOTFILES: &str = "/home/u/dotfiles/efr/config.toml";

fn protected() -> Locations {
    locations().with_write_sealed_root(CONFIG).unwrap().with_write_sealed_root(DOTFILES).unwrap()
}

fn engine_with(rules: Vec<Rule>) -> Engine {
    Engine::with_rules(protected(), Policy::new(rules).unwrap())
}

fn in_mode(mode: Mode, requirements: Requirements) -> DecisionInput {
    DecisionInput { mode, ..input(requirements, Scope::Project(app()), Origin::Shell) }
}

fn write(path: &str) -> Requirements {
    Requirements::none().with_write(path)
}

fn allow(action: Action, resource: Resource) -> Rule {
    Rule::new(action, resource, Effect::Allow)
}

#[rstest]
#[case::config_file("/home/u/.config/efr/config.toml")]
#[case::the_directory(CONFIG)]
#[case::the_registry("/home/u/.config/efr/projects.toml")]
#[case::a_new_file("/home/u/.config/efr/new.toml")]
#[case::the_dotfiles_target(DOTFILES)]
fn writing_efrs_config_is_denied_in_every_mode(#[case] path: &str) {
    let engine = engine_with(Vec::new());
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let decision = engine.decide(&in_mode(mode, write(path)));
        assert_eq!(decision.effect(), Effect::Deny, "{path} in {mode}");
        assert_eq!(decision.reasons()[0].cause, Cause::WriteSealed, "{path} in {mode}");
    }
}

#[rstest]
#[case::under_config(allow(Action::Write, Resource::Under("~/.config".into())))]
#[case::under_the_dir(allow(Action::Write, Resource::Under("~/.config/efr".into())))]
#[case::under_dotfiles(allow(Action::Write, Resource::Under("~/dotfiles".into())))]
#[case::class(allow(Action::Write, Resource::Class(PathClass::UserConfig)))]
#[case::any(allow(Action::Any, Resource::Any))]
fn no_user_rule_opens_efrs_config_for_writing(#[case] rule: Rule) {
    let engine = engine_with(vec![rule.clone()]);
    for path in ["/home/u/.config/efr/config.toml", DOTFILES] {
        let decision = engine.decide(&in_mode(Mode::Auto, write(path)));
        assert_eq!(decision.effect(), Effect::Deny, "{path} with {rule:?}");
    }
}

#[test]
fn no_conversation_rule_opens_efrs_config_for_writing() {
    let mut input = in_mode(Mode::Auto, write("/home/u/.config/efr/config.toml"));
    input.conversation_policy = ConversationPolicy::new(SCRATCH)
        .with_rules(Policy::new(vec![allow(Action::Any, Resource::Any)]).unwrap());
    assert_eq!(engine_with(Vec::new()).decide(&input).effect(), Effect::Deny);
}

#[test]
fn reading_efrs_config_is_free() {
    let engine = engine_with(Vec::new());
    for path in ["/home/u/.config/efr/config.toml", DOTFILES] {
        let read = Requirements::none().with_read(path);
        assert_eq!(engine.decide(&in_mode(Mode::Cautious, read)).effect(), Effect::Allow);
        let tree = Requirements::none().with_read_tree(CONFIG);
        assert_eq!(engine.decide(&in_mode(Mode::Auto, tree)).effect(), Effect::Allow);
    }
}

#[test]
fn a_write_above_efrs_config_asks_even_when_a_rule_allows_it() {
    let engine = engine_with(vec![allow(Action::Write, Resource::Under("~/.config".into()))]);
    let decision = engine.decide(&in_mode(Mode::Auto, write("/home/u/.config")));
    assert_eq!(decision.effect(), Effect::Ask);
    assert_eq!(decision.reasons()[0].cause, Cause::ReachesWriteSealed { root: CONFIG.into() });
    // A sibling stays the rule's to decide.
    let nvim = engine.decide(&in_mode(Mode::Auto, write("/home/u/.config/nvim/init.lua")));
    assert_eq!(nvim.effect(), Effect::Allow);
}

/// A shell line with what the shell tool declares for it, run in the project.
fn line(text: &str, writes: &[&str]) -> Requirements {
    let mut requirements =
        Requirements::none().with_command(text).with_command_dir("/home/u/p/app");
    for path in writes {
        requirements = requirements.with_write(*path);
    }
    requirements
}

#[rstest]
#[case::cp("cp x ~/.config/efr/config.toml", &["/home/u/.config/efr/config.toml"])]
#[case::cp_into_the_dir("cp -r x ~/.config/efr", &["/home/u/.config/efr"])]
#[case::ln_over("ln -sf ~/p/app/evil ~/.config/efr/config.toml", &["/home/u/.config/efr/config.toml"])]
#[case::mv_over("mv x ~/.config/efr/projects.toml", &["/home/u/p/app/x", "/home/u/.config/efr/projects.toml"])]
#[case::rm("rm ~/.config/efr/config.toml", &["/home/u/.config/efr/config.toml"])]
#[case::tee("tee ~/dotfiles/efr/config.toml", &[DOTFILES])]
#[case::redirection("echo x > ~/.config/efr/config.toml", &["/home/u/.config/efr/config.toml"])]
// A link in the project that leads to the config: the daemon declares what it reaches.
#[case::through_a_link("echo x > cfg/config.toml", &["/home/u/p/app/cfg/config.toml", "/home/u/.config/efr/config.toml"])]
fn shell_tricks_that_write_efrs_config_are_denied(#[case] text: &str, #[case] writes: &[&str]) {
    let engine = engine_with(Vec::new());
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let decision = engine.decide(&in_mode(mode, line(text, writes)));
        assert_eq!(decision.effect(), Effect::Deny, "{text:?} in {mode}");
    }
}

#[test]
fn a_write_sealed_root_under_a_home_alias_is_stored_under_the_home_directory() {
    let locations = Locations::new("/home/u")
        .unwrap()
        .with_write_sealed_root("/var/home/u/.config/efr")
        .unwrap()
        .with_home_alias("/var/home/u")
        .unwrap();
    let engine = Engine::with_rules(locations, Policy::empty());
    for path in ["/home/u/.config/efr/config.toml", "/var/home/u/.config/efr/config.toml"] {
        let decision = engine.decide(&in_mode(Mode::Auto, write(path)));
        assert_eq!(decision.effect(), Effect::Deny, "{path}");
    }
}
