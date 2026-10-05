use efr_permissions::{Action, CommandPattern, Effect, Policy, Resource, Rule};
use pretty_assertions::assert_eq;

use crate::shells::trusted_programs;

#[test]
fn the_trusted_programs_are_every_program_a_command_rule_names_once() {
    let user = Policy::new(vec![
        Rule::new(
            Action::Execute,
            Resource::Command(CommandPattern::new("cargo").with_args(["test"])),
            Effect::Allow,
        ),
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("ls")), Effect::Deny),
        Rule::new(Action::Read, Resource::Any, Effect::Allow),
    ])
    .unwrap();

    let programs = trusted_programs(&Policy::defaults().then(user));

    assert_eq!(programs.first().map(String::as_str), Some("ls"));
    assert_eq!(programs.last().map(String::as_str), Some("cargo"));
    for program in ["cat", "git", "systemctl", "ps", "jq", "find"] {
        assert!(programs.iter().any(|known| known == program), "{program} in {programs:?}");
    }
    let mut once = programs.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), programs.len(), "{programs:?}");
}

#[test]
fn a_policy_without_command_rules_trusts_no_program() {
    let policy = Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Ask)]).unwrap();
    assert_eq!(trusted_programs(&policy), Vec::<String>::new());
}
