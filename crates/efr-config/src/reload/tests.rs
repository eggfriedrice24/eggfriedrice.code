use std::path::Path;

use efr_stdx::env::Var;
use pretty_assertions::assert_eq;

use crate::{RESTART_KEYS, ScreenChoice, Settings, Source};

const PATH: &str = "/c/efr/config.toml";

fn file(text: &str) -> Settings {
    Settings::parse(Path::new(PATH), Some(text)).unwrap()
}

#[test]
fn a_live_key_takes_its_new_value() {
    let running = file("[shell]\nidle_minutes = 5\n");
    let next = file("[shell]\nidle_minutes = 30\n[model]\nname = \"gpt-5.4\"\n");

    let reloaded = running.reloaded(next.clone()).unwrap();

    assert_eq!(reloaded.settings, next);
    assert_eq!(reloaded.restart_needed, Vec::<String>::new());
    assert_eq!(reloaded.settings.source("model.name"), Source::File);
}

#[test]
fn a_restart_key_keeps_its_running_value_and_is_listed() {
    let running = file("[shell]\nidle_minutes = 5\n");
    let next = file(
        "screen = \"vt100\"\n[shell]\nidle_minutes = 30\n[openai]\nsubscription_base_url = \"http://127.0.0.1:9/x\"\n",
    );

    let reloaded = running.reloaded(next).unwrap();

    assert_eq!(reloaded.restart_needed, ["screen", "openai.subscription_base_url"]);
    assert_eq!(reloaded.settings.screen, ScreenChoice::Auto);
    assert_eq!(reloaded.settings.openai.subscription_base_url, None);
    assert_eq!(reloaded.settings.shell.idle_minutes, 30, "a live key still applies");
    assert_eq!(reloaded.settings.source("screen"), Source::Default);
}

#[test]
fn a_restart_key_set_back_to_its_running_value_needs_no_restart() {
    let running = file("screen = \"vt100\"\n");
    let next = file("screen = \"vt100\"\nlog = \"debug\"\n");

    let reloaded = running.reloaded(next).unwrap();

    assert_eq!(reloaded.restart_needed, Vec::<String>::new());
    assert_eq!(reloaded.settings.log, "debug");
}

#[test]
fn a_removed_restart_key_keeps_its_running_value() {
    let running = file("[model]\nprovider = \"openai-api\"\n");

    let reloaded = running.reloaded(file("")).unwrap();

    assert_eq!(reloaded.restart_needed, ["model.provider"]);
    assert_eq!(reloaded.settings.model.provider, "openai-api");
    assert_eq!(reloaded.settings.source("model.provider"), Source::File);
}

#[test]
fn every_restart_key_is_kept_when_it_changes() {
    let changed = [
        ("screen", "ghostty"),
        ("model.provider", "openai-api"),
        ("openai.originator", "someone-else"),
        ("openai.subscription_base_url", "https://example.com/codex"),
        ("openai.api_base_url", "https://example.com/v1"),
        ("openai.websocket", "off"),
        ("anthropic.base_url", "https://example.com/anthropic/v1"),
        ("anthropic.cache_ttl", "1h"),
        ("anthropic.workspace_id", "wrkspc_01AbC"),
    ];
    assert_eq!(changed.map(|(key, _)| key), RESTART_KEYS);
    for (key, value) in changed {
        let running = Settings::default();
        let mut next = Settings::default();
        next.apply_override(key, value, Source::File).unwrap();

        let reloaded = running.reloaded(next).unwrap();

        assert_eq!(reloaded.restart_needed, [key]);
        assert_eq!(reloaded.settings, running, "{key} keeps its running value");
    }
}

#[test]
fn a_value_from_a_variable_or_a_flag_still_wins_over_the_file() {
    let mut running = file("log = \"info\"\n");
    running.apply_override("log", "trace", Source::Flag("--log")).unwrap();
    running.apply_override("screen", "vt100", Source::Env(Var::Screen)).unwrap();
    let next = file("log = \"warn\"\nscreen = \"ghostty\"\n[shell]\nlogin = false\n");

    let reloaded = running.reloaded(next).unwrap();

    assert_eq!(reloaded.settings.log, "trace");
    assert_eq!(reloaded.settings.source("log"), Source::Flag("--log"));
    assert_eq!(reloaded.settings.screen, ScreenChoice::Vt100);
    assert_eq!(reloaded.settings.source("screen"), Source::Env(Var::Screen));
    assert_eq!(reloaded.restart_needed, Vec::<String>::new(), "the override never changed");
    assert!(!reloaded.settings.shell.login);
    assert_eq!(reloaded.settings.source("shell.login"), Source::File);
}

#[test]
fn the_rules_of_the_new_file_apply() {
    let rule = "[[permissions.rules]]\naction = \"execute\"\nresource = { command = { program = \"cargo\" } }\neffect = \"allow\"\n";
    let running = file("screen = \"vt100\"\n");
    let next = file(&format!("screen = \"ghostty\"\n{rule}"));

    let reloaded = running.reloaded(next.clone()).unwrap();

    assert_eq!(reloaded.settings.permissions.rules, next.permissions.rules);
    assert_eq!(reloaded.settings.permissions.rules.rules().len(), 1);
}
