use std::path::PathBuf;

use efr_config::{Settings, SudoCache};
use efr_permissions::{Action, CommandPattern, Effect, Policy, Resource, Rule};
use efr_protocol::Mode;
use pretty_assertions::assert_eq;

use super::{loosening, loosens};

fn rules(effect: Effect) -> Policy {
    let rule = Rule::new(Action::Execute, Resource::Command(CommandPattern::new("cargo")), effect);
    Policy::new(vec![rule]).unwrap()
}

#[test]
fn loosening_lists_each_key_that_loosens_in_order() {
    let before = Settings::default();
    let mut next = Settings::default();
    next.permissions.rules = rules(Effect::Allow);
    next.permissions.allow_full = true;
    next.permissions.mode = Mode::Full;
    next.shell.sudo_cache = SudoCache::Keep;
    next.sandbox.write_roots.push(PathBuf::from("~/notes"));
    next.jobs.max_minutes += 1;

    let keys = loosening(&before, &next);

    let mut expected = vec!["permissions.rules", "permissions.mode", "permissions.allow_full"];
    if before.shell.sudo_cache == SudoCache::PerCall {
        expected.push("shell.sudo_cache");
    }
    expected.extend(["sandbox.write_roots", "jobs.max_minutes"]);
    assert_eq!(keys, expected);
    assert!(loosens(&before, &next));
    assert_eq!(loosening(&next, &before), Vec::<&str>::new(), "the way back asks more");
}

#[test]
fn a_new_ask_rule_or_a_removed_allow_rule_does_not_loosen() {
    let mut allow = Settings::default();
    allow.permissions.rules = rules(Effect::Allow);
    let mut ask = Settings::default();
    ask.permissions.rules = rules(Effect::Ask);

    assert_eq!(loosening(&Settings::default(), &ask), Vec::<&str>::new());
    assert_eq!(loosening(&allow, &Settings::default()), Vec::<&str>::new());
    assert_eq!(loosening(&ask, &Settings::default()), ["permissions.rules"], "a removed ask");
}
