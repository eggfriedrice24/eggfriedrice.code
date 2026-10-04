use pretty_assertions::assert_eq;
use rstest::rstest;
use serde::{Deserialize, Serialize};

use super::{Action, CommandPattern, Policy, Resource, Rule};
use crate::{Effect, PathClass, PermissionsError};

#[test]
fn the_defaults_pass_their_own_checks() {
    let defaults = Policy::defaults();
    assert_eq!(Policy::new(defaults.rules().to_vec()), Ok(defaults));
}

#[test]
fn then_appends_later_rules_that_win() {
    let later = Policy::new(vec![Rule::new(Action::Read, Resource::Any, Effect::Deny)]).unwrap();
    let policy = Policy::defaults().then(later);
    assert_eq!(policy.rules().len(), 9);
    assert_eq!(policy.rules()[8], Rule::new(Action::Read, Resource::Any, Effect::Deny));
}

#[test]
fn push_checks_the_rule_at_its_position() {
    let mut policy = Policy::defaults();
    let bad = Rule::new(Action::Write, Resource::Under("relative".into()), Effect::Allow);
    assert_eq!(
        policy.push(bad),
        Err(PermissionsError::RulePathNotAbsolute { index: 8, path: "relative".into() })
    );
    policy.push(Rule::new(Action::Network, Resource::Any, Effect::Allow)).unwrap();
    assert_eq!(policy.rules().len(), 9);
}

#[rstest]
#[case::program_alone("ls", "ls", &[], true)]
#[case::extra_args("ls", "ls -la /etc", &[], true)]
#[case::tabs_and_spaces("git", " \tgit\t status ", &["status"], true)]
#[case::args_prefix("git", "git status --short", &["status"], true)]
#[case::other_args("git", "git push", &["status"], false)]
#[case::too_short("git", "git", &["status"], false)]
#[case::similar_program("ls", "lsblk", &[], false)]
#[case::path_program("ls", "/bin/ls", &[], false)]
#[case::option_with_value("git", "git log --format=%H -n1", &["log"], true)]
#[case::semicolon("ls", "ls; rm -rf ~", &[], false)]
#[case::and("ls", "ls && rm -rf x", &[], false)]
#[case::pipe("ls", "ls | sh", &[], false)]
#[case::redirect("ls", "ls > /etc/passwd", &[], false)]
#[case::substitution("ls", "ls $(rm -rf x)", &[], false)]
#[case::backticks("ls", "ls `rm -rf x`", &[], false)]
#[case::variable("ls", "ls $HOME", &[], false)]
#[case::tilde("ls", "ls ~", &[], false)]
#[case::glob("ls", "ls *", &[], false)]
#[case::quote("ls", "ls 'a b'", &[], false)]
#[case::newline("ls", "ls\nrm -rf x", &[], false)]
#[case::background("ls", "ls &", &[], false)]
#[case::assignment_first("git", "GIT_PAGER=x git status", &[], false)]
#[case::unicode("ls", "ls \u{e9}t\u{e9}", &[], false)]
#[case::empty("ls", "", &[], false)]
fn command_patterns_match_simple_lines_only(
    #[case] program: &str,
    #[case] line: &str,
    #[case] args: &[&str],
    #[case] expected: bool,
) {
    let pattern = CommandPattern::new(program).with_args(args.iter().copied());
    assert_eq!(pattern.matches(line), expected, "{line:?}");
}

#[rstest]
#[case::relative_under(
    Rule::new(Action::Write, Resource::Under("p/app".into()), Effect::Allow),
    PermissionsError::RulePathNotAbsolute { index: 0, path: "p/app".into() }
)]
#[case::other_users_home(
    Rule::new(Action::Write, Resource::Under("~bob/x".into()), Effect::Allow),
    PermissionsError::RulePathNotAbsolute { index: 0, path: "~bob/x".into() }
)]
#[case::empty_program(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: String::new() }
)]
#[case::program_with_space(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("git status")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "git status".into() }
)]
#[case::program_is_an_option(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("-rf")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "-rf".into() }
)]
#[case::program_is_an_assignment(
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("=ls")), Effect::Allow),
    PermissionsError::RuleProgramInvalid { index: 0, program: "=ls".into() }
)]
#[case::argument_with_glob(
    Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("rm").with_args(["*"])),
        Effect::Allow
    ),
    PermissionsError::RuleArgumentInvalid { index: 0, argument: "*".into() }
)]
#[case::execute_a_class(
    Rule::new(Action::Execute, Resource::Class(PathClass::System), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Execute }
)]
#[case::network_under(
    Rule::new(Action::Network, Resource::Under("/etc".into()), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Network }
)]
#[case::write_a_command(
    Rule::new(Action::Write, Resource::Command(CommandPattern::new("ls")), Effect::Allow),
    PermissionsError::RuleNeverMatches { index: 0, action: Action::Write }
)]
fn invalid_rules_are_refused(#[case] rule: Rule, #[case] expected: PermissionsError) {
    assert_eq!(Policy::new(vec![rule]), Err(expected));
}

#[test]
fn the_error_names_the_position_of_the_rule() {
    let rules = vec![
        Rule::new(Action::Any, Resource::Any, Effect::Ask),
        Rule::new(Action::Execute, Resource::Project, Effect::Allow),
    ];
    assert_eq!(
        Policy::new(rules).unwrap_err().to_string(),
        "rule 1 pairs the action execute with a resource that it never matches"
    );
}

/// The shape a configuration file gives the rules.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    rules: Policy,
}

#[test]
fn rules_read_from_toml() {
    let text = r#"
rules = [
    { action = "write", resource = { under = "~/.config/nvim" }, effect = "allow" },
    { action = "execute", resource = { command = { program = "git", args = ["status"] } }, effect = "allow" },
    { action = "any", resource = { class = "user_data" }, effect = "deny" },
    { action = "write", resource = "project", effect = "ask" },
    { action = "network", resource = "any", effect = "allow" },
]
"#;
    let config: Config = toml::from_str(text).unwrap();
    let expected = Policy::new(vec![
        Rule::new(Action::Write, Resource::Under("~/.config/nvim".into()), Effect::Allow),
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("git").with_args(["status"])),
            Effect::Allow,
        ),
        Rule::new(Action::Any, Resource::Class(PathClass::UserData), Effect::Deny),
        Rule::new(Action::Write, Resource::Project, Effect::Ask),
        Rule::new(Action::Network, Resource::Any, Effect::Allow),
    ])
    .unwrap();
    assert_eq!(config.rules, expected);
}

#[test]
fn rules_round_trip_through_toml() {
    let config = Config { rules: Policy::defaults() };
    let text = toml::to_string(&config).unwrap();
    assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
}

#[test]
fn reading_checks_the_rules() {
    let text =
        r#"rules = [{ action = "execute", resource = { class = "system" }, effect = "allow" }]"#;
    let error = toml::from_str::<Config>(text).unwrap_err().to_string();
    assert!(error.contains("rule 0 pairs the action execute"), "{error}");
}

#[rstest]
#[case::unknown_rule_key(
    r#"rules = [{ action = "read", resource = "any", effect = "allow", why = "x" }]"#
)]
#[case::unknown_pattern_key(
    r#"rules = [{ action = "execute", resource = { command = { program = "ls", flags = [] } }, effect = "allow" }]"#
)]
#[case::unknown_effect(r#"rules = [{ action = "read", resource = "any", effect = "maybe" }]"#)]
#[case::unknown_class(
    r#"rules = [{ action = "read", resource = { class = "home" }, effect = "allow" }]"#
)]
fn unknown_keys_and_values_are_refused(#[case] text: &str) {
    assert!(toml::from_str::<Config>(text).is_err());
}
