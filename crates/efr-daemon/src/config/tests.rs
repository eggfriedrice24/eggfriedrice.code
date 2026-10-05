use std::path::{Path, PathBuf};

use efr_config::{ConfigError, ScreenChoice, Settings, Source};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::config::{Flags, load_settings, resolve_settings};

const PATH: &str = "/home/u/.config/efr/config.toml";

fn path() -> &'static Path {
    Path::new(PATH)
}

fn no_env() -> Env {
    Env::fixed(Vec::<(Var, &str)>::new())
}

#[test]
fn without_a_file_or_overrides_every_value_is_the_default() {
    let settings = resolve_settings(path(), None, &no_env(), &Flags::default()).unwrap();

    assert_eq!(settings, Settings::parse(path(), None).unwrap());
    assert_eq!(settings.source("log"), Source::Default);
}

#[test]
fn the_environment_beats_the_file_and_flags_beat_the_environment() {
    let text = "log = \"warn\"\nscreen = \"ghostty\"\n";
    let env = Env::fixed([(Var::Log, "debug"), (Var::Screen, "vt100")]);

    let from_env = resolve_settings(path(), Some(text), &env, &Flags::default()).unwrap();
    assert_eq!(from_env.log, "debug");
    assert_eq!(from_env.screen, ScreenChoice::Vt100);
    assert_eq!(from_env.source("log"), Source::Env(Var::Log));

    let flags = Flags::new(Some("trace".to_owned()), Some("auto".to_owned()));
    let from_flags = resolve_settings(path(), Some(text), &env, &flags).unwrap();
    assert_eq!(from_flags.log, "trace");
    assert_eq!(from_flags.screen, ScreenChoice::Auto);
    assert_eq!(from_flags.source("screen"), Source::Flag("--screen"));
}

#[test]
fn a_bad_screen_from_the_environment_or_a_flag_is_refused() {
    let env = Env::fixed([(Var::Screen, "kitty")]);
    let result = resolve_settings(path(), None, &env, &Flags::default());
    assert!(
        matches!(&result, Err(DaemonError::Config { source: ConfigError::InvalidOverride { key, .. } }) if key == "screen"),
        "{result:?}"
    );

    let flags = Flags::new(None, Some("kitty".to_owned()));
    let result = resolve_settings(path(), None, &no_env(), &flags);
    assert!(matches!(result, Err(DaemonError::Config { .. })), "{result:?}");
}

#[test]
fn an_error_in_the_file_names_the_rule_or_key() {
    let text = "[[permissions.rules]]\naction = \"execute\"\nresource = { class = \"system\" }\neffect = \"allow\"\n";

    let error = resolve_settings(path(), Some(text), &no_env(), &Flags::default()).unwrap_err();

    assert_eq!(error.to_string(), "the config could not be loaded");
    let DaemonError::Config { source } = error else { panic!("{error:?}") };
    assert_eq!(source.to_string(), format!("permissions.rules[0] in {PATH} is invalid"));
    let result =
        resolve_settings(path(), Some("[shell]\nidle_minuets = 5\n"), &no_env(), &Flags::default());
    assert!(matches!(result, Err(DaemonError::Config { source: ConfigError::Parse { .. } })));
}

#[test]
fn a_missing_file_loads_as_the_defaults() {
    let dir = tempfile::tempdir().unwrap();

    let settings = load_settings(dir.path(), &no_env(), &Flags::default()).unwrap();

    let path = dir.path().join("config.toml");
    assert_eq!(settings, Settings::parse(&path, None).unwrap());
    assert_eq!(settings.path, path);
}

#[test]
fn a_file_that_cannot_be_read_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config.toml")).unwrap();

    let result = load_settings(dir.path(), &no_env(), &Flags::default());

    assert!(matches!(result, Err(DaemonError::Config { source: ConfigError::Read { .. } })));
}

#[test]
fn the_model_effort_is_read_where_the_openai_reasoning_effort_was() {
    let settings = resolve_settings(
        path(),
        Some("[model]\neffort = \"high\"\n"),
        &no_env(),
        &Flags::default(),
    )
    .unwrap();
    assert_eq!(settings.model.effort.as_deref(), Some("high"));

    let old = "[openai]\nreasoning_effort = \"high\"\n";
    let result = resolve_settings(path(), Some(old), &no_env(), &Flags::default());
    assert!(matches!(result, Err(DaemonError::Config { .. })), "the old key is gone: {result:?}");
}

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
    let env = Env::fixed([(Var::Log, "debug")]);
    let flags = Flags::new(None, Some("vt100".to_owned()));

    let config = resolve_settings(path(), Some(text), &env, &flags).unwrap();

    assert_eq!(config.path, PathBuf::from(PATH));
    insta::assert_snapshot!(config.effective());
}
