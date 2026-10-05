use efr_protocol::{Mode, TurnSettings};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::{Asked, Given, SettingSource};
use crate::cli::{Command, TurnSettingsArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::run;
use crate::testing::{TestEnv, command};

fn args(line: &[&str]) -> TurnSettingsArgs {
    match command(line) {
        Command::Settings(args) => args,
        other => panic!("not settings: {other:?}"),
    }
}

fn context(env: &TestEnv, vars: &[(Var, &str)]) -> Context {
    Context { env: Env::fixed(vars.iter().copied()), ..env.context() }
}

#[test]
fn without_flags_or_variables_nothing_is_asked() {
    let env = TestEnv::new();
    let asked = Asked::read(&env.context(), &args(&["settings"])).unwrap();
    assert_eq!(asked, Asked::default());
    assert!(asked.to_wire().is_empty());
}

#[test]
fn the_variables_ask_for_the_settings() {
    let env = TestEnv::new();
    let ctx = context(&env, &[(Var::Mode, "auto"), (Var::Model, "gpt-5.4"), (Var::Effort, "high")]);
    let asked = Asked::read(&ctx, &args(&["settings"])).unwrap();
    assert_eq!(
        asked.mode,
        Some(Given { value: Mode::Auto, source: SettingSource::Var(Var::Mode) })
    );
    assert_eq!(
        asked.to_wire(),
        TurnSettings {
            mode: Some(Mode::Auto),
            model: Some("gpt-5.4".to_owned()),
            effort: Some("high".to_owned()),
        }
    );
}

#[test]
fn a_flag_wins_over_its_variable() {
    let env = TestEnv::new();
    let ctx = context(&env, &[(Var::Mode, "auto"), (Var::Model, "gpt-5.4"), (Var::Effort, "high")]);
    let line = ["settings", "--mode", "manual", "--model", "gpt-5.5", "--effort", "low"];
    let asked = Asked::read(&ctx, &args(&line)).unwrap();
    assert_eq!(
        asked.mode,
        Some(Given { value: Mode::Manual, source: SettingSource::Flag("--mode") })
    );
    assert_eq!(
        asked.model,
        Some(Given { value: "gpt-5.5".to_owned(), source: SettingSource::Flag("--model") })
    );
    assert_eq!(
        asked.effort,
        Some(Given { value: "low".to_owned(), source: SettingSource::Flag("--effort") })
    );
}

#[test]
fn an_empty_variable_counts_as_unset() {
    let env = TestEnv::new();
    let ctx = context(&env, &[(Var::Mode, ""), (Var::Model, ""), (Var::Effort, "")]);
    let asked = Asked::read(&ctx, &args(&["settings"])).unwrap();
    assert_eq!(asked, Asked::default());
}

#[tokio::test]
async fn an_unknown_mode_in_the_variable_is_a_usage_error_with_the_choices() {
    let env = TestEnv::new();
    let ctx = context(&env, &[(Var::Mode, "yolo")]);
    let error = Asked::read(&ctx, &args(&["settings"])).unwrap_err();
    assert!(matches!(error, CliError::UnknownMode { input: Var::Mode, .. }), "{error:?}");
    assert_eq!(
        run::message(&error),
        "efr: EFR_MODE names the mode \"yolo\", which does not exist; choose one of: manual, \
         cautious, auto\n"
    );
    assert_eq!(error.exit(), crate::error::Exit::Usage);
}

#[test]
fn each_source_says_where_the_value_comes_from() {
    let shown: Vec<String> = [
        SettingSource::Flag("--model"),
        SettingSource::Var(Var::Effort),
        SettingSource::File("/home/user/.config/efr/config.toml".into()),
        SettingSource::Default,
        SettingSource::DaemonDefault,
        SettingSource::ModelDefault { model: "gpt-5.5".to_owned() },
        SettingSource::Backend { model: "gpt-5.4".to_owned() },
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        shown,
        [
            "--model",
            "EFR_EFFORT",
            "/home/user/.config/efr/config.toml",
            "default",
            "the daemon's default",
            "the default of gpt-5.5",
            "none is sent; the backend chooses for gpt-5.4",
        ]
    );
}
