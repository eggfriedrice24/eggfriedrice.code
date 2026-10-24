//! Rules for `git` read past git's own options before the subcommand (efr's v0.0.4 live
//! items plan, DL20 to DL26): a rule that asks or denies skips every one of them, in
//! every mode; a rule that allows skips only the plain ones.

use efr_protocol::{Mode, Origin, Scope};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::auto::auto_locations;
use super::{SCRATCH, app, input};
use crate::command::git::{Form, OPTIONS};
use crate::{
    Action, Cause, CommandPattern, ConversationPolicy, Decision, Effect, Engine, Layer, Policy,
    Requirements, Resource, Rule, Subject,
};

const APP: &str = "/home/u/p/app";

const MODES: [Mode; 4] = [Mode::Manual, Mode::Cautious, Mode::Auto, Mode::Full];

/// The lines of DL31 that run `git push` behind git's own options.
const PUSH_LINES: &[&str] = &[
    "git -C /home/u/app push origin HEAD",
    "git -c k=v push",
    "git --git-dir=/x/.git push",
    "git --git-dir /x/.git push",
    "git --work-tree=/x push",
    "git --work-tree /x push",
    "git --namespace=n push",
    "git --namespace n push",
    "git --exec-path=/x push",
    "git --config-env=k=V push",
    "git --config-env k=V push",
    "git --attr-source HEAD push",
    "git --attr-source=HEAD push",
    "git --shallow-file f push",
    "git -p push",
    "git --paginate push",
    "git -P push",
    "git --no-pager push",
    "git --bare push",
    "git --no-replace-objects push",
    "git --no-lazy-fetch push",
    "git --literal-pathspecs push",
    "git --no-literal-pathspecs push",
    "git --glob-pathspecs push",
    "git --noglob-pathspecs push",
    "git --icase-pathspecs push",
    "git --no-optional-locks push",
    "git --no-advice push",
    "git -C a -C b -c x=y --no-pager push origin HEAD",
    // An alias of the line, read as its command (DL26).
    "git -c alias.p=push p",
    "git -c alias.P=push p",
    "git -c 'alias.p=!git push' p",
    "git --config-env=alias.p=X p",
];

fn pattern(args: &[&str]) -> CommandPattern {
    CommandPattern::new("git").with_args(args.iter().copied())
}

fn rule(pattern: CommandPattern, effect: Effect) -> Rule {
    Rule::new(Action::Execute, Resource::Command(pattern), effect)
}

/// The engine of the `auto` machine with the built-in rules and then `rules`.
fn with_rules(rules: Vec<Rule>) -> Engine {
    Engine::with_rules(auto_locations(), Policy::new(rules).unwrap())
}

/// A shell call of `line` in `dir` that reaches the network, in the project's scope.
fn shell_in(line: &str, dir: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(dir).with_network()
}

fn decide_in(engine: &Engine, mode: Mode, requirements: Requirements) -> Decision {
    let mut input = input(requirements, Scope::Project(app()), Origin::Shell);
    input.mode = mode;
    engine.decide(&input)
}

/// The effect and the cause of the command line's reason.
fn command_reason(decision: &Decision) -> (Effect, Cause) {
    let reason = decision
        .reasons()
        .iter()
        .find(|reason| matches!(reason.subject, Subject::Command { .. }))
        .unwrap();
    (reason.effect, reason.cause.clone())
}

/// The cause of the last rule of the machine policy of `mode`: the user's rule.
fn user_rule(engine: &Engine, mode: Mode) -> Cause {
    Cause::Rule { layer: Layer::Machine, index: engine.policy(mode).rules().len() - 1 }
}

/// The cause of the rule before the last one of the machine policy of `mode`.
fn user_rule_before_last(engine: &Engine, mode: Mode) -> Cause {
    Cause::Rule { layer: Layer::Machine, index: engine.policy(mode).rules().len() - 2 }
}

#[rstest]
fn a_tightening_rule_matches_after_gits_own_options(
    #[values(Effect::Ask, Effect::Deny)] effect: Effect,
) {
    let engine = with_rules(vec![rule(pattern(&["push"]), effect)]);
    for mode in MODES {
        for line in PUSH_LINES {
            let decision = decide_in(&engine, mode, shell_in(line, APP));
            assert_eq!(decision.effect(), effect, "{line:?} in {mode:?}");
            assert_eq!(
                command_reason(&decision),
                (effect, user_rule(&engine, mode)),
                "{line:?} in {mode:?}"
            );
        }
    }
}

#[rstest]
#[case::value_of_dir("git -C push status", &[])]
#[case::alias_of_another_name("git -c alias.x=push status", &[])]
#[case::operand("git log --grep push", &[])]
#[case::forbidden_word("git -C x push --dry-run", &["--dry-run"])]
fn a_tightening_rule_skips_options_not_operands(#[case] line: &str, #[case] forbid: &[&str]) {
    let push = pattern(&["push"]).with_forbid(forbid.iter().copied());
    for effect in [Effect::Ask, Effect::Deny] {
        let engine = with_rules(vec![rule(push.clone(), effect)]);
        for mode in MODES {
            let decision = decide_in(&engine, mode, shell_in(line, APP));
            assert_ne!(command_reason(&decision).1, user_rule(&engine, mode), "{line:?} {mode:?}");
        }
    }
}

#[rstest]
#[case::no_pager("git --no-pager log", true)]
#[case::no_pager_short("git -P log", true)]
#[case::plain_options("git --no-optional-locks --no-advice log -1", true)]
#[case::raw("git log", true)]
#[case::dir("git -C /x log", false)]
#[case::config("git -c core.pager=x log", false)]
#[case::exec_path("git --exec-path=/x log", false)]
#[case::paginate("git -p log", false)]
#[case::git_dir("git --git-dir=/x/.git log", false)]
#[case::plain_then_dir("git --no-pager -C /x log", false)]
#[case::unknown("git --zz log", false)]
fn an_allow_rule_skips_only_the_plain_options(#[case] line: &str, #[case] matches: bool) {
    let engine = with_rules(vec![rule(pattern(&["log"]), Effect::Allow)]);
    for mode in [Mode::Manual, Mode::Cautious] {
        let decision = decide_in(&engine, mode, Requirements::none().with_command(line));
        let by_user = command_reason(&decision).1 == user_rule(&engine, mode);
        assert_eq!(by_user, matches, "{line:?} in {mode:?}");
        let expected = if matches { Effect::Allow } else { Effect::Ask };
        assert_eq!(decision.effect(), expected, "{line:?} in {mode:?}");
    }
}

#[test]
fn last_match_still_wins() {
    // `allow git`, then `deny git push`: the deny rule reads the view.
    let engine =
        with_rules(vec![rule(pattern(&[]), Effect::Allow), rule(pattern(&["push"]), Effect::Deny)]);
    let decision = decide_in(&engine, Mode::Cautious, shell_in("git -C x push", APP));
    assert_eq!(decision.effect(), Effect::Deny);
    // `ask git push`, then `allow git push origin main`: the allow rule reads only the
    // raw words, so the question stays.
    let engine = with_rules(vec![
        rule(pattern(&["push"]), Effect::Ask),
        rule(pattern(&["push", "origin", "main"]), Effect::Allow),
    ]);
    let line = "git -C x push origin main";
    let decision = decide_in(&engine, Mode::Cautious, Requirements::none().with_command(line));
    assert_eq!(decision.effect(), Effect::Ask);
    let decision = decide_in(
        &engine,
        Mode::Cautious,
        Requirements::none().with_command("git push origin main"),
    );
    assert_eq!(decision.effect(), Effect::Allow);
}

#[test]
fn a_rule_that_reads_through_a_wrapper_or_a_view_only_makes_it_stricter() {
    // `deny nice`, then `ask git push`: the ask rule matches `nice git push` only
    // through the wrapper, so the deny stays.
    let engine = with_rules(vec![
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("nice")), Effect::Deny),
        rule(pattern(&["push"]), Effect::Ask),
    ]);
    for mode in [Mode::Manual, Mode::Cautious] {
        let decision = decide_in(&engine, mode, Requirements::none().with_command("nice git push"));
        assert_eq!(command_reason(&decision), (Effect::Deny, user_rule_before_last(&engine, mode)));
    }
    // `deny git -C`, then `ask git push`: the ask rule matches `git -C x push` only
    // through the git view, so the deny stays.
    let engine = with_rules(vec![
        rule(pattern(&["-C"]), Effect::Deny),
        rule(pattern(&["push"]), Effect::Ask),
    ]);
    for mode in [Mode::Manual, Mode::Cautious] {
        let decision = decide_in(&engine, mode, Requirements::none().with_command("git -C x push"));
        assert_eq!(command_reason(&decision), (Effect::Deny, user_rule_before_last(&engine, mode)));
    }
    // `allow nice`, then `ask git push`: the ask rule is stricter, so it decides.
    let engine = with_rules(vec![
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("nice")), Effect::Allow),
        rule(pattern(&["push"]), Effect::Ask),
    ]);
    let decision =
        decide_in(&engine, Mode::Cautious, Requirements::none().with_command("nice git push"));
    assert_eq!(command_reason(&decision), (Effect::Ask, user_rule(&engine, Mode::Cautious)));
    // `deny git push`, then `ask nice`: the ask rule matches the words as they are, so
    // the last match wins, as before.
    let engine = with_rules(vec![
        rule(pattern(&["push"]), Effect::Deny),
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("nice")), Effect::Ask),
    ]);
    let decision =
        decide_in(&engine, Mode::Cautious, Requirements::none().with_command("nice git push"));
    assert_eq!(command_reason(&decision), (Effect::Ask, user_rule(&engine, Mode::Cautious)));
}

#[rstest]
fn a_rule_with_a_global_option_in_its_args_matches_that_exact_line(
    #[values(Effect::Allow, Effect::Ask, Effect::Deny)] effect: Effect,
) {
    let engine = with_rules(vec![rule(pattern(&["-C", "/home/u/p/app", "push"]), effect)]);
    let line = "git -C /home/u/p/app push origin HEAD";
    for mode in [Mode::Manual, Mode::Cautious] {
        let decision = decide_in(&engine, mode, Requirements::none().with_command(line));
        assert_eq!(command_reason(&decision), (effect, user_rule(&engine, mode)), "{mode:?}");
    }
}

#[rstest]
#[case::dir_below_the_root("git -C ~/work/app push", "/home/u", true)]
#[case::shell_below_the_root("git -C /tmp push", "/home/u/work/app", true)]
#[case::work_tree_below_the_root("git --work-tree=/home/u/work/x push", "/srv", true)]
#[case::relative_dir("git -C app push", "/home/u/work", true)]
#[case::chained_dirs("git -C /home/u -C work push", "/srv", true)]
#[case::alias_dir("git -c 'alias.p=-C /home/u/work push' p", "/srv", true)]
#[case::neither("git -C /tmp push", "/srv", false)]
#[case::dir_outside_the_root("git -C /srv push", "/home/u", false)]
fn under_of_a_tightening_rule_counts_gits_dirs(
    #[case] line: &str,
    #[case] dir: &str,
    #[case] denied: bool,
) {
    let deny = pattern(&["push"]).with_under("~/work");
    let engine = with_rules(vec![rule(deny, Effect::Deny)]);
    for mode in MODES {
        let decision = decide_in(&engine, mode, shell_in(line, dir));
        let by_user = command_reason(&decision).1 == user_rule(&engine, mode);
        assert_eq!(by_user, denied, "{line:?} from {dir} in {mode:?}");
    }
}

#[test]
fn under_reads_gits_dirs_from_each_dir_the_line_may_be_in() {
    let deny = pattern(&["push"]).with_under("~/work");
    let engine = with_rules(vec![rule(deny, Effect::Deny)]);
    // The `cd` may fail, so a relative `-C` counts from `~` too (OQL9).
    let line = "cd /srv && git -C work push";
    let decision = decide_in(&engine, Mode::Cautious, shell_in(line, "/home/u"));
    assert_eq!(decision.effect(), Effect::Deny);
    // An absolute `-C` is known.
    let line = "cd /srv && git -C /home/u/work push";
    let decision = decide_in(&engine, Mode::Cautious, shell_in(line, "/srv"));
    assert_eq!(decision.effect(), Effect::Deny);
    // From `/srv` and `/tmp`, `-C work` is outside `~/work`.
    let line = "cd /tmp && git -C work push";
    let decision = decide_in(&engine, Mode::Cautious, shell_in(line, "/srv"));
    assert_ne!(decision.effect(), Effect::Deny);
}

#[test]
fn a_conversation_rule_reads_the_same_views() {
    let engine = Engine::with_defaults(auto_locations());
    let ask = Policy::new(vec![rule(pattern(&["push"]), Effect::Ask)]).unwrap();
    for mode in MODES {
        let mut input = input(shell_in(PUSH_LINES[0], APP), Scope::Project(app()), Origin::Shell);
        input.mode = mode;
        input.conversation_policy = ConversationPolicy::new(SCRATCH).with_rules(ask.clone());
        let decision = engine.decide(&input);
        assert_eq!(
            command_reason(&decision),
            (Effect::Ask, Cause::Rule { layer: Layer::Conversation, index: 0 }),
            "{mode:?}"
        );
    }
}

#[test]
fn a_network_rule_reads_the_same_views() {
    let network = Rule::new(Action::Network, Resource::Command(pattern(&["push"])), Effect::Deny);
    let engine = with_rules(vec![network]);
    for line in ["git -C /x push", "git --no-pager push", "git -c alias.p=push p"] {
        let decision = decide_in(&engine, Mode::Cautious, shell_in(line, APP));
        assert_eq!(decision.effect(), Effect::Deny, "{line:?}");
        let network = decision
            .reasons()
            .iter()
            .find(|reason| matches!(reason.subject, Subject::Network))
            .unwrap();
        assert_eq!(network.cause, user_rule(&engine, Mode::Cautious), "{line:?}");
    }
}

/// Lines that run `git push` behind a wrapper or by a path (OQL3).
const WRAPPED_PUSH_LINES: &[&str] = &[
    "env git -C /home/u/p/app push origin HEAD",
    "env GIT_TRACE=1 git push",
    "nice git push",
    "nice -n 5 git push",
    "timeout 60 git push",
    "command git push",
    "nohup git push",
    "stdbuf -oL git push",
    "ionice -c 3 git push",
    "setsid git push",
    "xargs git push",
    "/usr/bin/git push",
    "/usr/bin/git -C /home/u/p/app push",
    "env nice /usr/bin/git --no-pager push",
    "/usr/bin/env git push",
];

#[rstest]
fn a_tightening_rule_matches_behind_wrappers_and_paths(
    #[values(Effect::Ask, Effect::Deny)] effect: Effect,
) {
    let engine = with_rules(vec![rule(pattern(&["push"]), effect)]);
    for mode in MODES {
        for line in WRAPPED_PUSH_LINES {
            let decision = decide_in(&engine, mode, shell_in(line, APP));
            assert_eq!(decision.effect(), effect, "{line:?} in {mode:?}");
            assert_eq!(
                command_reason(&decision),
                (effect, user_rule(&engine, mode)),
                "{line:?} in {mode:?}"
            );
        }
    }
}

#[test]
fn a_tightening_rule_for_another_program_matches_behind_wrappers_and_paths() {
    let rm = Resource::Command(CommandPattern::new("rm"));
    let engine = with_rules(vec![Rule::new(Action::Execute, rm, Effect::Deny)]);
    for line in ["/bin/rm x", "xargs rm", "nice -n 5 rm -f x", "env rm x"] {
        let decision = decide_in(&engine, Mode::Cautious, shell_in(line, APP));
        assert_eq!(decision.effect(), Effect::Deny, "{line:?}");
    }
    let decision = decide_in(&engine, Mode::Cautious, shell_in("nice cargo build", APP));
    assert_ne!(decision.effect(), Effect::Deny);
}

#[rstest]
#[case::path("/usr/bin/git status")]
#[case::wrapper("env git status")]
#[case::wrapper_value("nice -n 5 git status")]
fn an_allow_rule_still_reads_only_the_program_itself(#[case] line: &str) {
    // DL27: a path is another program, and a wrapper is the program that runs.
    let engine = with_rules(vec![rule(pattern(&["status"]), Effect::Allow)]);
    let decision = decide_in(&engine, Mode::Manual, Requirements::none().with_command(line));
    assert_eq!(decision.effect(), Effect::Ask, "{line:?}");
}

#[test]
fn under_reads_the_dir_of_a_wrapped_git() {
    let deny = pattern(&["push"]).with_under("~/work");
    let engine = with_rules(vec![rule(deny, Effect::Deny)]);
    let decision = decide_in(&engine, Mode::Cautious, shell_in("env git -C ~/work/a push", "/srv"));
    assert_eq!(decision.effect(), Effect::Deny);
    let decision = decide_in(&engine, Mode::Cautious, shell_in("env git -C /tmp push", "/srv"));
    assert_ne!(decision.effect(), Effect::Deny);
}

/// One global option of the table with a value where it takes one.
fn any_global() -> impl Strategy<Value = (String, bool)> {
    let values = prop::sample::select(vec!["x", "/tmp/x", "k=v", "HEAD"]);
    (prop::sample::select(OPTIONS.to_vec()), values).prop_map(|(option, value)| {
        let plain = option.class == crate::command::git::Class::Plain;
        let text = match option.form {
            Form::Next => format!("{} {value}", option.name),
            Form::Joined => format!("{}={value}", option.name),
            Form::Flag => option.name.to_owned(),
        };
        (text, plain)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn global_options_never_hide_a_push_from_an_ask_rule(
        prefix in prop::collection::vec(any_global(), 0..6),
    ) {
        let words: Vec<String> = prefix.iter().map(|(text, _)| text.clone()).collect();
        let line = format!("git {} push", words.join(" "));
        let engine = with_rules(vec![rule(pattern(&["push"]), Effect::Ask)]);
        let decision = decide_in(&engine, Mode::Cautious, shell_in(&line, APP));
        prop_assert_eq!(command_reason(&decision), (Effect::Ask, user_rule(&engine, Mode::Cautious)));
    }

    #[test]
    fn only_plain_options_let_an_allow_rule_match(
        prefix in prop::collection::vec(any_global(), 1..6),
    ) {
        let words: Vec<String> = prefix.iter().map(|(text, _)| text.clone()).collect();
        let plain = prefix.iter().all(|(_, plain)| *plain);
        let line = format!("git {} push", words.join(" "));
        let engine = with_rules(vec![rule(pattern(&["push"]), Effect::Allow)]);
        let decision = decide_in(&engine, Mode::Manual, Requirements::none().with_command(&line));
        let by_user = command_reason(&decision).1 == user_rule(&engine, Mode::Manual);
        prop_assert_eq!(by_user, plain, "{}", line);
    }
}
