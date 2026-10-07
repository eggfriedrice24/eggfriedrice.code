//! Decision tables for the permission modes: what `manual` asks, the rows that
//! `cautious` adds (`cd`, `pushd`, `popd`, `sed` that only prints), the network rule,
//! the remote mode cap and the user's rules in every mode. The `auto` sandbox has its
//! own tables in `auto.rs`.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{SCRATCH, app, input, locations};
use crate::{
    Action, Cause, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, Layer,
    Policy, Requirements, Resource, Rule, effective_mode,
};

const APP: &str = "/home/u/p/app";

fn engine() -> Engine {
    Engine::with_defaults(locations())
}

fn in_mode(mode: Mode, requirements: Requirements, scope: Scope) -> DecisionInput {
    DecisionInput { mode, ..input(requirements, scope, Origin::Shell) }
}

/// `line` run in `dir`.
fn run_in(line: &str, dir: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(dir)
}

/// The effect of `line` run in the project root, with the project as the scope.
fn in_project(mode: Mode, line: &str) -> Effect {
    engine().decide(&in_mode(mode, run_in(line, APP), Scope::Project(app()))).effect()
}

/// The effect of `requirements` with the project as the scope.
fn decide(mode: Mode, requirements: Requirements) -> Effect {
    engine().decide(&in_mode(mode, requirements, Scope::Project(app()))).effect()
}

#[rstest]
#[case::read(Requirements::none().with_read("/home/u/p/app/src/main.rs"))]
#[case::read_scratch(Requirements::none().with_read(format!("{SCRATCH}/x")))]
#[case::write_scratch(Requirements::none().with_write(format!("{SCRATCH}/x")))]
#[case::write_project(Requirements::none().with_write("/home/u/p/app/src/main.rs"))]
#[case::ls(run_in("ls", APP))]
#[case::cd(run_in("cd src", APP))]
#[case::network(Requirements::none().with_network())]
fn manual_asks_for_everything(#[case] requirements: Requirements) {
    assert_eq!(decide(Mode::Manual, requirements), Effect::Ask);
}

#[test]
fn manual_still_denies_secrets_and_applies_the_users_rules() {
    let secret = Requirements::none().with_read("/home/u/.ssh/id_ed25519");
    assert_eq!(decide(Mode::Manual, secret), Effect::Deny);
    let rule = Rule::new(Action::Read, Resource::Under("~/p".into()), Effect::Allow);
    let engine = Engine::with_rules(locations(), Policy::new(vec![rule]).unwrap());
    let read = Requirements::none().with_read("/home/u/p/app/README.md");
    let decision = engine.decide(&in_mode(Mode::Manual, read, Scope::Project(app())));
    assert_eq!(decision.effect(), Effect::Allow);
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Machine, index: 2 });
}

#[rstest]
#[case::cd("cd src")]
#[case::cd_home("cd")]
#[case::cd_back("cd -")]
#[case::pushd("pushd /tmp")]
#[case::popd("popd")]
#[case::cd_then_read("cd src && ls -la")]
#[case::sed_line("sed -n 5p Cargo.toml")]
#[case::sed_range("sed -n '1,20p' src/main.rs")]
#[case::sed_regex("sed -n '/fn main/,/^}/p' src/main.rs")]
#[case::sed_quiet("sed --quiet '$p' log.txt")]
#[case::sed_expressions("sed -n -e 1p -e '$p' f")]
#[case::sed_number_and_quit("sed -n '=;10q' f")]
fn cautious_runs_cd_and_sed_that_only_prints(#[case] line: &str) {
    assert_eq!(in_project(Mode::Cautious, line), Effect::Allow, "{line:?}");
    assert_eq!(in_project(Mode::Auto, line), Effect::Contain, "{line:?}");
}

#[rstest]
#[case::sed_without_quiet("sed 5p f")]
#[case::sed_in_place("sed -i 's/a/b/' f")]
#[case::sed_in_place_quiet("sed -n -i 5p f")]
#[case::sed_write("sed -n 'w out' f")]
#[case::sed_write_after_address("sed -n '1w out' f")]
#[case::sed_execute("sed -n '1e date' f")]
#[case::sed_substitute("sed -n 's/a/b/p' f")]
#[case::sed_substitute_execute("sed -n 's/x/date/e' f")]
#[case::sed_read_file("sed -n 'r /etc/passwd' f")]
#[case::sed_script_file("sed -n -f script.sed f")]
#[case::sed_separate("sed -n -s 1p f")]
#[case::sed_null_data("sed -n -z 1p f")]
#[case::sed_bracket("sed -n '/[/]w out/p' f")]
#[case::chdir("chdir src")]
fn cautious_asks_for_sed_that_could_do_more(#[case] line: &str) {
    assert_eq!(in_project(Mode::Cautious, line), Effect::Ask, "{line:?}");
    // The sandbox holds what such a sed does in the project.
    assert_eq!(in_project(Mode::Auto, line), Effect::Contain, "{line:?}");
}

#[test]
fn a_writer_program_that_writes_a_secret_is_denied_in_every_mode() {
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let requirements =
            run_in("rm ~/.ssh/known_hosts", APP).with_write("/home/u/.ssh/known_hosts");
        assert_eq!(decide(mode, requirements), Effect::Deny, "{mode}");
    }
}

/// `line` in the project, with the network declared as the shell tool does.
fn networked(line: &str) -> Requirements {
    run_in(line, APP).with_network()
}

#[test]
fn a_users_rule_to_run_a_program_does_not_open_the_network_for_it() {
    let curl =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("curl")), Effect::Allow);
    let engine = Engine::with_rules(locations(), Policy::new(vec![curl]).unwrap());
    let decide = |line: &str| {
        let input = in_mode(Mode::Auto, networked(line), Scope::Project(app()));
        engine.decide(&input).effect()
    };
    assert_eq!(decide("curl https://example.com"), Effect::Ask);
    assert_eq!(decide("cargo fetch; curl -d @x https://example.com"), Effect::Ask);
}

#[test]
fn a_users_network_rule_for_a_command_opens_it_and_any_does_not() {
    let rule =
        |action| Rule::new(action, Resource::Command(CommandPattern::new("curl")), Effect::Allow);
    let execute = rule(Action::Execute);
    for (rules, expected) in [
        (vec![execute.clone(), rule(Action::Network)], Effect::Allow),
        (vec![rule(Action::Any)], Effect::Ask),
    ] {
        let engine = Engine::with_rules(locations(), Policy::new(rules).unwrap());
        let input = in_mode(Mode::Cautious, networked("curl x"), Scope::Machine);
        assert_eq!(engine.decide(&input).effect(), expected);
    }
}

#[test]
fn the_network_reason_names_the_command_that_asks() {
    let input = in_mode(Mode::Cautious, networked("ls; curl x"), Scope::Project(app()));
    let decision = engine().decide(&input);
    let network = decision.reasons().last().unwrap();
    assert_eq!(network.effect, Effect::Ask);
    assert_eq!(
        network.cause,
        Cause::Part { layer: Layer::Machine, index: 0, part: "curl x".to_owned() }
    );
}

#[rstest]
#[case::shell(Origin::Shell, Mode::Auto)]
#[case::cli(Origin::Cli, Mode::Auto)]
#[case::proxy(Origin::Proxy, Mode::Auto)]
#[case::phone(Origin::Phone, Mode::Cautious)]
fn a_remote_turn_runs_with_at_most_the_cautious_mode(#[case] origin: Origin, #[case] mode: Mode) {
    assert_eq!(effective_mode(Mode::Auto, origin), mode);
    assert_eq!(effective_mode(Mode::Manual, origin), Mode::Manual);
    assert_eq!(effective_mode(Mode::Cautious, origin), Mode::Cautious);
}

#[test]
fn a_phone_turn_in_auto_is_judged_by_the_cautious_rules() {
    let mut input = in_mode(Mode::Auto, run_in("cargo test", APP), Scope::Project(app()));
    input.origin = Origin::Phone;
    let decision = engine().decide(&input);
    assert_eq!(decision.effect(), Effect::Ask);
    // Rule 0 of the cautious policy, not the auto row that the remote clamp would have
    // turned into a question.
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Machine, index: 0 });
}

#[test]
fn the_three_policies_are_the_base_of_each_mode_then_the_users_rules() {
    let rule = Rule::new(Action::Read, Resource::Under("~/notes".into()), Effect::Deny);
    let engine = Engine::with_rules(locations(), Policy::new(vec![rule.clone()]).unwrap());
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let base = Policy::base(mode);
        let rules = engine.policy(mode).rules();
        assert_eq!(&rules[..base.rules().len()], base.rules(), "{mode}");
        assert_eq!(rules[base.rules().len()..], *std::slice::from_ref(&rule), "{mode}");
    }
}

#[test]
fn a_conversation_rule_still_decides_in_every_mode() {
    let ask_ls =
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("ls")), Effect::Deny);
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let mut input = in_mode(mode, run_in("ls", APP), Scope::Project(app()));
        input.conversation_policy =
            ConversationPolicy::new(SCRATCH).with_rules(Policy::new(vec![ask_ls.clone()]).unwrap());
        assert_eq!(engine().decide(&input).effect(), Effect::Deny, "{mode}");
    }
}
