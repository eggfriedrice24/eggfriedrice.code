use std::path::{Path, PathBuf};

use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::config::{Config, Flags, ScreenChoice, Source};

const PATH: &str = "/home/u/.config/efr/config.toml";

fn path() -> &'static Path {
    Path::new(PATH)
}

fn no_env() -> Env {
    Env::fixed(Vec::<(Var, &str)>::new())
}

#[test]
fn without_a_file_every_value_is_the_default() {
    let config = Config::resolve(path(), None, &no_env(), &Flags::default()).unwrap();

    assert_eq!(config.log, "info");
    assert_eq!(config.screen, ScreenChoice::Auto);
    assert_eq!(config.provider, "openai-subscription");
    assert_eq!(config.model, None);
    assert!(config.shell.login);
    assert_eq!(config.shell.idle_minutes, 60);
    assert_eq!(config.conversation.max_queued, 16);
    assert_eq!(config.source("log"), Source::Default);
    assert_eq!(config.path, PathBuf::from(PATH));
}

#[test]
fn the_file_sets_what_it_names() {
    let text = r#"
        log = "warn"
        screen = "vt100"

        [model]
        provider = "openai-api"
        name = "gpt-6-sol"
        max_output_tokens = 4096

        [openai]
        originator = "efr-dev"
        models = ["gpt-6-sol", "gpt-5.5"]

        [shell]
        program = "/usr/bin/zsh"
        login = false
        idle_minutes = 0

        [conversation]
        max_queued = 4
        approval_timeout_secs = 600
        update_interval_ms = 50

        [permissions]
        secret_paths = ["~/.config/rclone/rclone.conf", "/srv/vault"]

        [render]
        theme = "ansi"
    "#;

    let config = Config::resolve(path(), Some(text), &no_env(), &Flags::default()).unwrap();

    assert_eq!(config.log, "warn");
    assert_eq!(config.screen, ScreenChoice::Vt100);
    assert_eq!(config.provider, "openai-api");
    assert_eq!(config.model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(config.max_output_tokens, Some(4096));
    assert_eq!(config.openai.originator, "efr-dev");
    assert_eq!(
        config.openai.models.as_deref(),
        Some(&["gpt-6-sol".to_owned(), "gpt-5.5".to_owned()][..])
    );
    assert_eq!(config.shell.program.as_deref(), Some(Path::new("/usr/bin/zsh")));
    assert!(!config.shell.login);
    assert_eq!(config.shell.idle_minutes, 0);
    assert_eq!(config.conversation.max_queued, 4);
    assert_eq!(config.conversation.approval_timeout_secs, Some(600));
    assert_eq!(config.conversation.update_interval_ms, 50);
    assert_eq!(
        config.permissions.secret_paths,
        [PathBuf::from("~/.config/rclone/rclone.conf"), PathBuf::from("/srv/vault")]
    );
    assert_eq!(config.render_theme.as_deref(), Some("ansi"));
    assert_eq!(config.source("model.name"), Source::File);
    assert_eq!(config.source("model.system_prompt"), Source::Default);
}

#[test]
fn the_environment_beats_the_file_and_flags_beat_the_environment() {
    let text = "log = \"warn\"\nscreen = \"ghostty\"\n";
    let env = Env::fixed([(Var::Log, "debug"), (Var::Screen, "vt100")]);

    let from_env = Config::resolve(path(), Some(text), &env, &Flags::default()).unwrap();
    assert_eq!(from_env.log, "debug");
    assert_eq!(from_env.screen, ScreenChoice::Vt100);
    assert_eq!(from_env.source("log"), Source::Env(Var::Log));

    let flags = Flags::new(Some("trace".to_owned()), Some("auto".to_owned()));
    let from_flags = Config::resolve(path(), Some(text), &env, &flags).unwrap();
    assert_eq!(from_flags.log, "trace");
    assert_eq!(from_flags.screen, ScreenChoice::Auto);
    assert_eq!(from_flags.source("screen"), Source::Flag("--screen"));
}

#[test]
fn an_unknown_key_is_an_error() {
    let result =
        Config::resolve(path(), Some("[shell]\nidle_minuets = 5\n"), &no_env(), &Flags::default());

    match result {
        Err(DaemonError::ParseConfig { path: at, source }) => {
            assert_eq!(at, PathBuf::from(PATH));
            assert!(source.to_string().contains("idle_minuets"), "{source}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unknown_table_is_an_error() {
    let result =
        Config::resolve(path(), Some("[mcp]\nservers = []\n"), &no_env(), &Flags::default());

    assert!(matches!(result, Err(DaemonError::ParseConfig { .. })), "{result:?}");
}

#[test]
fn values_outside_their_set_are_refused_with_the_key() {
    let cases = [
        ("screen = \"kitty\"\n", "screen"),
        ("[model]\nprovider = \"anthropic\"\n", "model.provider"),
        ("[shell]\nprogram = \"zsh\"\n", "shell.program"),
        ("[permissions]\nsecret_paths = [\"vault\"]\n", "permissions.secret_paths"),
    ];
    for (text, key) in cases {
        let result = Config::resolve(path(), Some(text), &no_env(), &Flags::default());
        assert!(
            matches!(&result, Err(DaemonError::InvalidConfig { key: found, .. }) if *found == key),
            "{text}: {result:?}"
        );
    }
}

#[test]
fn a_bad_screen_from_the_environment_is_refused_too() {
    let env = Env::fixed([(Var::Screen, "kitty")]);

    let result = Config::resolve(path(), None, &env, &Flags::default());

    assert!(matches!(result, Err(DaemonError::InvalidConfig { key: "screen", .. })), "{result:?}");
}

#[test]
fn a_missing_file_loads_as_the_defaults() {
    let dir = tempfile::tempdir().unwrap();

    let config = Config::load(dir.path(), &no_env(), &Flags::default()).unwrap();

    assert_eq!(config.path, dir.path().join("config.toml"));
    assert_eq!(config, Config { path: dir.path().join("config.toml"), ..Config::default() });
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
    "#;
    let env = Env::fixed([(Var::Log, "debug")]);
    let flags = Flags::new(None, Some("vt100".to_owned()));

    let config = Config::resolve(path(), Some(text), &env, &flags).unwrap();

    insta::assert_snapshot!(config.effective());
}
