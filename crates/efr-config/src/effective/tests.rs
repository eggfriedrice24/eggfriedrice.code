use std::path::Path;

use efr_permissions::{Action, CommandPattern, Effect, Policy, Resource, Rule};
use efr_stdx::env::Var;
use pretty_assertions::assert_eq;

use crate::{Settings, Source, keys};

const PATH: &str = "/home/u/.config/efr/config.toml";

#[test]
fn the_effective_dump_names_every_value_and_its_source() {
    let text = r#"
        [model]
        name = "gpt-6-sol"

        [shell]
        idle_minutes = 30

        [render]
        theme = "catppuccin-mocha"

        [[permissions.rules]]
        action = "execute"
        resource = { command = { program = "cargo", args = ["test"] } }
        effect = "allow"
    "#;
    let mut settings = Settings::parse(Path::new(PATH), Some(text)).unwrap();
    settings.apply_override("log", "debug", Source::Env(Var::Log)).unwrap();
    settings.apply_override("screen", "vt100", Source::Flag("--screen")).unwrap();

    insta::assert_snapshot!(settings.effective());
}

#[test]
fn the_dump_lists_every_key_once_in_the_order_of_the_keys() {
    let entries = Settings::default().entries();

    let listed: Vec<String> = entries.into_iter().map(|entry| entry.key).collect();
    assert_eq!(listed, keys());
}

#[test]
fn the_dump_prints_every_bound_of_a_command_rule() {
    let pattern = CommandPattern::new("ps")
        .with_args(["-ef"])
        .with_forbid(["-x"])
        .with_min_operands(0)
        .with_max_operands(0)
        .with_max_options(0)
        .with_under("~/p");
    let mut settings = Settings::default();
    settings.permissions.rules =
        Policy::new(vec![Rule::new(Action::Execute, Resource::Command(pattern), Effect::Allow)])
            .unwrap();

    let rules = settings.entries().into_iter().find(|entry| entry.key == "permissions.rules");

    assert_eq!(
        rules.map(|entry| entry.value).as_deref(),
        Some(
            "[{ action = \"execute\", resource = { command = { program = \"ps\", args = [\"-ef\"], \
             forbid = [\"-x\"], max_operands = 0, min_operands = 0, max_options = 0, under = \"~/p\" } }, \
             effect = \"allow\" }]"
        )
    );
}

#[test]
fn a_long_or_multi_line_string_is_shown_as_its_length() {
    let mut settings = Settings::default();
    settings.model.system_prompt = "one\ntwo".to_owned();

    let prompt = settings.entries().into_iter().find(|entry| entry.key == "model.system_prompt");

    assert_eq!(prompt.map(|entry| entry.value).as_deref(), Some("<7 bytes>"));
}
