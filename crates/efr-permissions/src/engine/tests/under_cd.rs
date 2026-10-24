//! `under` of a rule that asks or denies after a `cd` in the line (efr's v0.0.4 live
//! items plan, OQL9): where efr cannot prove that the command runs outside the rule's
//! dir, the rule applies, in every mode. A rule that allows keeps the dir where the
//! line starts.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::{app, input};
use crate::{
    Action, Cause, CommandPattern, Construct, Decision, Effect, Engine, Layer, Policy,
    Requirements, Resource, Rule, Subject,
};

const MODES: [Mode; 4] = [Mode::Manual, Mode::Cautious, Mode::Auto, Mode::Full];

fn rule(pattern: CommandPattern, effect: Effect) -> Rule {
    Rule::new(Action::Execute, Resource::Command(pattern), effect)
}

fn push_under_work() -> CommandPattern {
    CommandPattern::new("git").with_args(["push"]).with_under("~/work")
}

fn with_rules(rules: Vec<Rule>) -> Engine {
    Engine::with_rules(auto_locations(), Policy::new(rules).unwrap())
}

fn shell_in(line: &str, dir: Option<&str>) -> Requirements {
    let requirements = Requirements::none().with_command(line).with_network();
    match dir {
        Some(dir) => requirements.with_command_dir(dir),
        None => requirements,
    }
}

fn decide_in(engine: &Engine, mode: Mode, requirements: Requirements) -> Decision {
    let mut input = input(requirements, Scope::Project(app()), Origin::Shell);
    input.mode = mode;
    engine.decide(&input)
}

fn command_reason(decision: &Decision) -> (Effect, Cause) {
    let reason = decision
        .reasons()
        .iter()
        .find(|reason| matches!(reason.subject, Subject::Command { .. }))
        .unwrap();
    (reason.effect, reason.cause.clone())
}

/// The cause of the rule `back` places before the end of the machine policy of `mode`.
fn user_rule(engine: &Engine, mode: Mode, back: usize) -> Cause {
    Cause::Rule { layer: Layer::Machine, index: engine.policy(mode).rules().len() - 1 - back }
}

/// The cause of the command line when the part `git push` decides it by the rule
/// `back` places before the end.
fn decided_by(engine: &Engine, mode: Mode, line: &str, back: usize) -> Cause {
    match user_rule(engine, mode, back) {
        Cause::Rule { layer, index } if crate::command::analyze(line).unwrap().len() > 1 => {
            Cause::Part { layer, index, part: "git push".to_owned() }
        }
        cause => cause,
    }
}

#[rstest]
// The dir is known: `under` works as before.
#[case::outside("git push", Some("/srv"), false)]
#[case::inside("git push", Some("/home/u/work/app"), true)]
#[case::before_the_cd("git push; cd ~/work", Some("/srv"), false)]
// The dir is not known: the rule applies.
#[case::no_dir("git push", None, true)]
#[case::relative("cd sub && git push", Some("/srv"), true)]
#[case::up("cd .. && git push", Some("/srv"), true)]
#[case::back("cd - && git push", Some("/srv"), true)]
#[case::stack("cd +1 && git push", Some("/srv"), true)]
#[case::stack_minus("cd -2 && git push", Some("/srv"), true)]
#[case::replace("cd a b && git push", Some("/srv"), true)]
#[case::popd("popd && git push", Some("/srv"), true)]
#[case::bare_pushd("pushd && git push", Some("/srv"), true)]
#[case::pattern("cd /tmp/w* && git push", Some("/srv"), true)]
#[case::behind_time("time cd /tmp && git push", Some("/srv"), true)]
#[case::relative_after_a_known_cd("cd ~/x && cd y && git push", Some("/srv"), true)]
// A target that efr can read: the rule applies when it lies inside, or when the dir
// before the `cd` does, because the `cd` can fail.
#[case::into_the_dir("cd ~/work/app && git push", Some("/srv"), true)]
#[case::into_the_dir_by_path("cd -P -- /home/u/work && git push", Some("/srv"), true)]
#[case::pushd_into_the_dir("pushd ~/work && git push", Some("/srv"), true)]
#[case::chdir_into_the_dir("chdir /home/u/work && git push", Some("/srv"), true)]
#[case::earlier_into_the_dir("cd ~/work; cd /tmp; git push", Some("/srv"), true)]
#[case::out_of_the_dir("cd /tmp && git push", Some("/home/u/work/app"), true)]
#[case::elsewhere("cd /tmp && git push", Some("/srv"), false)]
#[case::elsewhere_twice("cd /tmp; cd /var && git push", Some("/srv"), false)]
#[case::home("cd && git push", Some("/srv"), false)]
#[case::home_tilde("cd ~ && git push", Some("/srv"), false)]
fn a_tightening_rule_with_under_applies_where_the_dir_is_not_proven_outside(
    #[case] line: &str,
    #[case] dir: Option<&str>,
    #[case] applies: bool,
    #[values(Effect::Ask, Effect::Deny)] effect: Effect,
) {
    // NOTE: the `cd` parts run freely, so only the `git push` part can ask.
    let mut rules: Vec<Rule> = ["cd", "chdir", "pushd", "popd", "time"]
        .into_iter()
        .map(|program| rule(CommandPattern::new(program), Effect::Allow))
        .collect();
    rules.push(rule(push_under_work(), effect));
    let engine = with_rules(rules);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in(line, dir));
        let (got, cause) = command_reason(&decision);
        let by_user = cause == decided_by(&engine, mode, line, 0);
        assert_eq!(by_user, applies, "{line:?} from {dir:?} in {mode:?}: {cause:?}");
        if applies {
            assert_eq!(got, effect, "{line:?} from {dir:?} in {mode:?}");
        }
    }
}

#[test]
fn a_network_rule_with_under_applies_after_a_cd_too() {
    let network = Rule::new(Action::Network, Resource::Command(push_under_work()), Effect::Deny);
    let engine = with_rules(vec![network]);
    let decision = decide_in(&engine, Mode::Cautious, shell_in("cd sub && git push", Some("/srv")));
    assert_eq!(decision.effect(), Effect::Deny);
    let decision =
        decide_in(&engine, Mode::Cautious, shell_in("cd /tmp && git push", Some("/srv")));
    assert_ne!(decision.effect(), Effect::Deny);
}

#[test]
fn a_match_after_a_cd_only_makes_the_effect_stricter() {
    // `deny git push`, then `ask git push` in `~/work`: after `cd sub` the ask rule
    // may hold, but it must not lift the denial that is sure.
    let engine = with_rules(vec![
        rule(CommandPattern::new("git").with_args(["push"]), Effect::Deny),
        rule(push_under_work(), Effect::Ask),
    ]);
    for mode in MODES {
        let line = "cd sub && git push";
        let decision = decide_in(&engine, mode, shell_in(line, Some("/srv")));
        assert_eq!(
            command_reason(&decision),
            (Effect::Deny, decided_by(&engine, mode, line, 1)),
            "{mode:?}"
        );
    }
    // `allow git`, then `ask git push` in `~/work`: the ask rule is stricter, so it
    // decides.
    let engine = with_rules(vec![
        rule(CommandPattern::new("git"), Effect::Allow),
        rule(push_under_work(), Effect::Ask),
    ]);
    let line = "cd sub && git push";
    let decision = decide_in(&engine, Mode::Cautious, shell_in(line, Some("/srv")));
    assert_eq!(
        command_reason(&decision),
        (Effect::Ask, decided_by(&engine, Mode::Cautious, line, 0))
    );
}

#[rstest]
#[case::in_the_dir("cargo test", "/home/u/p/app", Effect::Allow)]
#[case::into_the_dir("cd /home/u/p/app && cargo test", "/srv", Effect::Ask)]
#[case::relative("cd crates && cargo test", "/home/u/p/app", Effect::Ask)]
// zsh runs `time cd` in the shell itself, so the dir is no longer known.
#[case::behind_time("time cd /srv && cargo test", "/home/u/p/app", Effect::Ask)]
fn an_allow_rule_with_under_matches_only_before_a_cd(
    #[case] line: &str,
    #[case] dir: &str,
    #[case] expected: Effect,
) {
    let allow = CommandPattern::new("cargo").with_args(["test"]).with_under("~/p/app");
    let engine = with_rules(vec![
        rule(CommandPattern::new("cd"), Effect::Allow),
        rule(CommandPattern::new("time"), Effect::Allow),
        rule(allow, Effect::Allow),
    ]);
    let requirements = Requirements::none().with_command(line).with_command_dir(dir);
    let decision = decide_in(&engine, Mode::Cautious, requirements);
    assert_eq!(decision.effect(), expected, "{line:?} from {dir}");
}

/// The cause of a line that cannot be split, decided by the user's last rule.
fn opaque_by_user(engine: &Engine, mode: Mode, construct: Construct) -> Cause {
    let Cause::Rule { layer, index } = user_rule(engine, mode, 0) else { unreachable!() };
    Cause::Opaque { construct, layer, index }
}

#[rstest]
#[case::cd_to_a_variable("cd $DIR && git push", Construct::Expansion)]
#[case::cd_to_a_substitution("cd $(mktemp -d) && git push", Construct::CommandSubstitution)]
#[case::substitution_after("git push $(echo origin)", Construct::CommandSubstitution)]
#[case::assignment_before("X=$(pwd) git push", Construct::CommandSubstitution)]
#[case::inside_a_substitution("echo $(git push)", Construct::CommandSubstitution)]
#[case::in_a_group("(cd /tmp && git push)", Construct::Grouping)]
#[case::wrapped("x=1; env git -C /tmp push", Construct::Assignment)]
fn a_tightening_rule_reads_the_commands_of_a_line_that_cannot_be_split(
    #[case] line: &str,
    #[case] construct: Construct,
    #[values(Effect::Ask, Effect::Deny)] effect: Effect,
) {
    let engine = with_rules(vec![rule(push_under_work(), effect)]);
    for mode in MODES {
        // NOTE: where the line runs is unknown, so `under` holds even from `/srv`.
        let decision = decide_in(&engine, mode, shell_in(line, Some("/srv")));
        assert_eq!(
            command_reason(&decision),
            (effect, opaque_by_user(&engine, mode, construct.clone())),
            "{line:?} in {mode:?}"
        );
    }
}

#[test]
fn a_network_rule_reads_the_commands_of_a_line_that_cannot_be_split() {
    let network = Rule::new(Action::Network, Resource::Command(push_under_work()), Effect::Deny);
    let engine = with_rules(vec![network]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in("cd $DIR && git push", Some("/srv")));
        assert_eq!(decision.effect(), Effect::Deny, "{mode:?}");
    }
}

#[test]
fn a_rule_that_allows_reads_no_command_of_a_line_that_cannot_be_split() {
    let engine = with_rules(vec![rule(CommandPattern::new("ls"), Effect::Allow)]);
    let decision = decide_in(&engine, Mode::Cautious, shell_in("ls $(cat dirs)", Some("/srv")));
    assert_eq!(
        command_reason(&decision),
        (
            Effect::Ask,
            Cause::Opaque {
                construct: Construct::CommandSubstitution,
                layer: Layer::Machine,
                index: 0
            }
        )
    );
}

#[test]
fn a_command_of_a_line_that_cannot_be_split_only_makes_the_effect_stricter() {
    // A rule for every command line that denies stays: the ask rule is less strict.
    let engine = with_rules(vec![
        Rule::new(Action::Execute, Resource::Any, Effect::Deny),
        rule(CommandPattern::new("git").with_args(["push"]), Effect::Ask),
    ]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in("git push $(x)", Some("/srv")));
        let Cause::Rule { layer, index } = user_rule(&engine, mode, 1) else { unreachable!() };
        let construct = Construct::CommandSubstitution;
        assert_eq!(
            command_reason(&decision),
            (Effect::Deny, Cause::Opaque { construct, layer, index }),
            "{mode:?}"
        );
    }
}
