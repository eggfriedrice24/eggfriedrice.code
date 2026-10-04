use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rstest::rstest;
use serde::{Deserialize, Serialize};

use super::{Action, CommandPattern, MatchContext, Policy, Resource, Rule};
use crate::{Access, Effect, PathClass, PermissionsError, Subject};

const HOME: &str = "/home/u";

fn cx() -> MatchContext<'static> {
    MatchContext {
        home: Path::new(HOME),
        home_aliases: &[],
        project_root: Some(Path::new("/home/u/p/app")),
    }
}

fn write(path: &str, class: PathClass) -> Subject {
    Subject::Path { path: PathBuf::from(path), access: Access::Write, class: Some(class) }
}

fn read(path: &str, class: PathClass) -> Subject {
    Subject::Path { path: PathBuf::from(path), access: Access::Read, class: Some(class) }
}

fn command(line: &str) -> Subject {
    Subject::Command { line: line.to_owned() }
}

#[test]
fn the_defaults_pass_their_own_checks() {
    let defaults = Policy::defaults();
    assert_eq!(Policy::new(defaults.rules().to_vec()), Ok(defaults));
}

#[test]
fn the_last_matching_rule_wins() {
    let allow_then_deny = Policy::new(vec![
        Rule::new(Action::Write, Resource::Any, Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~/.config".into()), Effect::Deny),
    ])
    .unwrap();
    let subject = write("/home/u/.config/a", PathClass::UserConfig);
    assert_eq!(allow_then_deny.last_match(&subject, &cx()), Some((1, Effect::Deny)));

    let deny_then_allow =
        Policy::new(allow_then_deny.rules().iter().rev().cloned().collect()).unwrap();
    assert_eq!(deny_then_allow.last_match(&subject, &cx()), Some((1, Effect::Allow)));

    let other = write("/home/u/notes", PathClass::UserData);
    assert_eq!(allow_then_deny.last_match(&other, &cx()), Some((0, Effect::Allow)));
}

#[test]
fn an_empty_policy_matches_nothing() {
    assert_eq!(Policy::empty().last_match(&read("/etc/hosts", PathClass::System), &cx()), None);
}

#[test]
fn then_appends_later_rules_that_win() {
    let later = Policy::new(vec![Rule::new(Action::Read, Resource::Any, Effect::Deny)]).unwrap();
    let policy = Policy::defaults().then(later);
    assert_eq!(policy.rules().len(), 9);
    assert_eq!(
        policy.last_match(&read("/etc/hosts", PathClass::System), &cx()),
        Some((8, Effect::Deny))
    );
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
    assert_eq!(policy.last_match(&Subject::Network, &cx()), Some((8, Effect::Allow)));
}

#[rstest]
#[case::any_any(Action::Any, Resource::Any, true, true, true, true)]
#[case::read_any(Action::Read, Resource::Any, true, false, false, false)]
#[case::write_any(Action::Write, Resource::Any, false, true, false, false)]
#[case::execute_any(Action::Execute, Resource::Any, false, false, true, false)]
#[case::network_any(Action::Network, Resource::Any, false, false, false, true)]
#[case::any_class(Action::Any, Resource::Class(PathClass::UserData), true, true, false, false)]
#[case::any_under(Action::Any, Resource::Under("/home/u/p".into()), true, true, false, false)]
#[case::any_project(Action::Any, Resource::Project, true, true, false, false)]
#[case::any_command(
    Action::Any,
    Resource::Command(CommandPattern::new("ls")),
    false,
    false,
    true,
    false
)]
fn actions_and_resources_select_subjects(
    #[case] action: Action,
    #[case] resource: Resource,
    #[case] reads: bool,
    #[case] writes: bool,
    #[case] runs: bool,
    #[case] networks: bool,
) {
    let policy = Policy::new(vec![Rule::new(action, resource, Effect::Allow)]).unwrap();
    let matched = |subject: Subject| policy.last_match(&subject, &cx()).is_some();
    assert_eq!(matched(read("/home/u/p/app/a", PathClass::UserData)), reads);
    assert_eq!(matched(write("/home/u/p/app/a", PathClass::UserData)), writes);
    assert_eq!(matched(command("ls -la")), runs);
    assert_eq!(matched(Subject::Network), networks);
}

#[test]
fn engine_only_subjects_match_no_rule() {
    let policy = Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap();
    let relative = Subject::Path { path: PathBuf::from("a"), access: Access::Read, class: None };
    for subject in [relative, Subject::Interactive, Subject::Nothing] {
        assert_eq!(policy.last_match(&subject, &cx()), None, "{subject:?}");
    }
}

#[rstest]
#[case::inside("/home/u/.config/nvim/init.lua", true)]
#[case::the_root_itself("/home/u/.config/nvim", true)]
#[case::sibling_prefix("/home/u/.config/nvim-old/init.lua", false)]
#[case::parent("/home/u/.config", false)]
fn under_matches_by_component(#[case] path: &str, #[case] expected: bool) {
    for root in ["~/.config/nvim", "/home/u/.config/nvim", "/home/u/.config/./nvim/"] {
        let policy = Policy::new(vec![Rule::new(
            Action::Write,
            Resource::Under(root.into()),
            Effect::Allow,
        )])
        .unwrap();
        let matched = policy.last_match(&write(path, PathClass::UserConfig), &cx()).is_some();
        assert_eq!(matched, expected, "{root} against {path}");
    }
}

#[test]
fn a_bare_tilde_is_the_home_directory() {
    let policy =
        Policy::new(vec![Rule::new(Action::Write, Resource::Under("~".into()), Effect::Allow)])
            .unwrap();
    assert!(policy.last_match(&write("/home/u/a", PathClass::UserData), &cx()).is_some());
    assert!(policy.last_match(&write("/etc/a", PathClass::System), &cx()).is_none());
}

#[test]
fn project_matches_only_inside_the_widening_root() {
    let policy =
        Policy::new(vec![Rule::new(Action::Write, Resource::Project, Effect::Allow)]).unwrap();
    let inside = write("/home/u/p/app/src/main.rs", PathClass::UserData);
    let outside = write("/home/u/p/other/a", PathClass::UserData);
    assert!(policy.last_match(&inside, &cx()).is_some());
    assert!(policy.last_match(&outside, &cx()).is_none());
    let no_project = MatchContext { home: Path::new(HOME), home_aliases: &[], project_root: None };
    assert!(policy.last_match(&inside, &no_project).is_none());
}

#[test]
fn under_and_project_rules_match_both_forms_of_a_linked_home() {
    let policy = Policy::new(vec![
        Rule::new(Action::Write, Resource::Under("/var/home/u/.config/nvim".into()), Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~/.config/zsh".into()), Effect::Allow),
        Rule::new(Action::Write, Resource::Project, Effect::Ask),
    ])
    .unwrap();
    let aliases = [PathBuf::from("/var/home/u")];
    let cx = MatchContext {
        home: Path::new(HOME),
        home_aliases: &aliases,
        project_root: Some(Path::new("/home/u/p/app")),
    };
    let rule = |path: &str| policy.last_match(&write(path, PathClass::UserConfig), &cx);
    assert_eq!(rule("/home/u/.config/nvim/init.lua"), Some((0, Effect::Allow)));
    assert_eq!(rule("/var/home/u/.config/nvim/init.lua"), Some((0, Effect::Allow)));
    assert_eq!(rule("/var/home/u/.config/zsh/aliases.zsh"), Some((1, Effect::Allow)));
    assert_eq!(rule("/var/home/u/p/app/src/main.rs"), Some((2, Effect::Ask)));
    assert_eq!(rule("/var/home/u/.config/fish/config.fish"), None);
    assert_eq!(rule("/var/home/u2/.config/nvim/init.lua"), None);
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
