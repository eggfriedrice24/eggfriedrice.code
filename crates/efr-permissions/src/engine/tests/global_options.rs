//! Rules for `systemctl`, `cargo`, `docker` and `kubectl` read past the program's own
//! options before the subcommand (efr's v0.0.4 live items plan, OQL4): a rule that asks
//! or denies skips each of them, in every mode; a rule that allows skips none.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::{app, input};
use crate::{
    Action, Cause, CommandPattern, Decision, Effect, Engine, Layer, Policy, Requirements, Resource,
    Rule, Subject,
};

const APP: &str = "/home/u/p/app";

const MODES: [Mode; 4] = [Mode::Manual, Mode::Cautious, Mode::Auto, Mode::Full];

fn rule(pattern: CommandPattern, effect: Effect) -> Rule {
    Rule::new(Action::Execute, Resource::Command(pattern), effect)
}

fn with_rules(rules: Vec<Rule>) -> Engine {
    Engine::with_rules(auto_locations(), Policy::new(rules).unwrap())
}

fn shell_in(line: &str, dir: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(dir).with_network()
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

/// The rule's program and `args`, and a line that runs it behind the program's options.
#[rstest]
#[case::systemctl_user("systemctl restart", "systemctl --user restart nginx")]
#[case::systemctl_host("systemctl restart", "systemctl -H web1 restart nginx")]
#[case::systemctl_host_joined("systemctl restart", "systemctl --host=web1 restart nginx")]
#[case::systemctl_cluster("systemctl restart", "systemctl -qH web1 restart nginx")]
#[case::systemctl_several("systemctl restart", "systemctl --user --no-block -M c restart x")]
#[case::systemctl_unknown("systemctl restart", "systemctl --zz restart nginx")]
#[case::systemctl_abbreviation("systemctl restart", "systemctl --us restart nginx")]
#[case::systemctl_by_path("systemctl restart", "/usr/bin/systemctl --user restart nginx")]
#[case::systemctl_wrapped("systemctl restart", "env systemctl --user restart nginx")]
#[case::cargo_nightly("cargo publish", "cargo +nightly publish")]
#[case::cargo_toolchain("cargo publish", "cargo +1.91.0 publish --dry-run")]
#[case::cargo_options("cargo publish", "cargo +stable -vv --locked --offline publish")]
#[case::cargo_color("cargo publish", "cargo --color always publish")]
#[case::cargo_config("cargo publish", "cargo --config net.offline=true publish")]
#[case::cargo_unstable("cargo publish", "cargo -Z unstable-options publish")]
#[case::cargo_dir("cargo publish", "cargo -C crates/x publish")]
#[case::cargo_alias("cargo publish", "cargo --config alias.p=publish p")]
#[case::cargo_config_file("cargo publish", "cargo --config extra.toml p")]
#[case::docker_host("docker push", "docker -H ssh://prod push app:1")]
#[case::docker_host_joined("docker push", "docker --host=tcp://h:2376 push app:1")]
#[case::docker_context("docker push", "docker --context prod push app:1")]
#[case::docker_short_context("docker push", "docker -c prod push app:1")]
#[case::docker_tls("docker push", "docker --tlsverify --tlscacert ca.pem push app:1")]
#[case::docker_debug("docker push", "docker -D -l debug push app:1")]
#[case::kubectl_namespace("kubectl delete", "kubectl -n prod delete pod web")]
#[case::kubectl_namespace_joined("kubectl delete", "kubectl --namespace=prod delete pod web")]
#[case::kubectl_context("kubectl delete", "kubectl --context prod delete pod web")]
#[case::kubectl_kubeconfig("kubectl delete", "kubectl --kubeconfig /k/c delete pod web")]
#[case::kubectl_level("kubectl delete", "kubectl -v 6 -n prod delete pod web")]
#[case::kubectl_kuberc("kubectl delete", "kubectl --kuberc k.yaml rm pod web")]
fn a_tightening_rule_matches_after_the_programs_own_options(
    #[case] wanted: &str,
    #[case] line: &str,
    #[values(Effect::Ask, Effect::Deny)] effect: Effect,
) {
    let (program, args) = wanted.split_once(' ').unwrap();
    let engine = with_rules(vec![rule(CommandPattern::new(program).with_args([args]), effect)]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in(line, APP));
        assert_eq!(
            command_reason(&decision),
            (effect, user_rule(&engine, mode, 0)),
            "{line:?} in {mode:?}"
        );
    }
}

#[rstest]
// The value of an option is no subcommand.
#[case::value_of_host("systemctl restart", "systemctl -H restart status nginx", &[])]
#[case::value_of_context("docker push", "docker --context push ps", &[])]
#[case::value_of_namespace("kubectl delete", "kubectl -n delete get pods", &[])]
#[case::value_of_dir("cargo publish", "cargo -C publish build", &[])]
// An operand is no subcommand.
#[case::operand("cargo publish", "cargo test --features publish", &[])]
#[case::later_word("systemctl restart", "systemctl status restart", &[])]
// A forbidden word after the subcommand still keeps the rule out.
#[case::forbidden("cargo publish", "cargo +nightly publish --dry-run", &["--dry-run"])]
fn a_tightening_rule_skips_options_not_values_or_operands(
    #[case] wanted: &str,
    #[case] line: &str,
    #[case] forbid: &[&str],
) {
    let (program, args) = wanted.split_once(' ').unwrap();
    let pattern =
        CommandPattern::new(program).with_args([args]).with_forbid(forbid.iter().copied());
    for effect in [Effect::Ask, Effect::Deny] {
        let engine = with_rules(vec![rule(pattern.clone(), effect)]);
        for mode in MODES {
            let decision = decide_in(&engine, mode, shell_in(line, APP));
            let cause = command_reason(&decision).1;
            assert_ne!(cause, user_rule(&engine, mode, 0), "{line:?} in {mode:?}");
        }
    }
}

#[rstest]
#[case::systemctl("systemctl restart nginx", "systemctl --user restart nginx")]
#[case::cargo("cargo test", "cargo +nightly test")]
#[case::docker("docker ps", "docker -H ssh://prod ps")]
#[case::kubectl("kubectl get", "kubectl --context prod get pods")]
fn an_allow_rule_skips_no_option(#[case] wanted: &str, #[case] line: &str) {
    let mut words = wanted.split(' ');
    let program = words.next().unwrap();
    let engine =
        with_rules(vec![rule(CommandPattern::new(program).with_args(words), Effect::Allow)]);
    for mode in [Mode::Manual, Mode::Cautious] {
        let requirements = Requirements::none().with_command(line);
        let decision = decide_in(&engine, mode, requirements.clone());
        assert_eq!(decision.effect(), Effect::Ask, "{line:?} in {mode:?}");
        let requirements = Requirements::none().with_command(wanted);
        let decision = decide_in(&engine, mode, requirements);
        assert_eq!(decision.effect(), Effect::Allow, "{wanted:?} in {mode:?}");
    }
}

#[test]
fn a_match_past_the_options_only_makes_the_effect_stricter() {
    // `deny docker`, then `ask docker push`: the ask rule matches `docker -H h push`
    // only past the options, so the deny stays.
    let engine = with_rules(vec![
        rule(CommandPattern::new("docker"), Effect::Deny),
        rule(CommandPattern::new("docker").with_args(["push"]), Effect::Ask),
    ]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in("docker -H h push app", APP));
        assert_eq!(command_reason(&decision), (Effect::Deny, user_rule(&engine, mode, 1)));
    }
}

#[rstest]
#[case::dir_inside("cargo -C ~/work/app publish", "/srv", true)]
#[case::relative_dir_inside("cargo -C app publish", "/home/u/work", true)]
#[case::dir_outside("cargo -C /tmp publish", "/srv", false)]
#[case::shell_inside("cargo -C /tmp publish", "/home/u/work/app", true)]
fn under_of_a_tightening_rule_counts_cargos_dir(
    #[case] line: &str,
    #[case] dir: &str,
    #[case] denied: bool,
) {
    let deny = CommandPattern::new("cargo").with_args(["publish"]).with_under("~/work");
    let engine = with_rules(vec![rule(deny, Effect::Deny)]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in(line, dir));
        let by_user = command_reason(&decision).1 == user_rule(&engine, mode, 0);
        assert_eq!(by_user, denied, "{line:?} from {dir} in {mode:?}");
    }
}
