use std::path::{Path, PathBuf};

use efr_protocol::{ErrorBody, ErrorCode, Method, Mode, Origin};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::{Line, resolve, show};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::run;
use crate::settings::{Settings, TurnDefaults};
use crate::testing::{TestEnv, capture, command, models};
use crate::turn_settings::{Asked, Given, SettingSource};

const PATH: &str = "/home/user/.config/efr/config.toml";

fn file() -> SettingSource {
    SettingSource::File(PathBuf::from(PATH))
}

fn defaults(mode: Option<Mode>, model: Option<&str>, effort: Option<&str>) -> TurnDefaults {
    TurnDefaults {
        path: Some(PathBuf::from(PATH)),
        mode,
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    }
}

fn given(value: &str, source: SettingSource) -> Option<Given<String>> {
    Some(Given { value: value.to_owned(), source })
}

/// The value and the source of each line.
fn values(lines: &[Line]) -> Vec<(&'static str, Option<String>, SettingSource)> {
    lines.iter().map(|line| (line.key, line.value.clone(), line.source.clone())).collect()
}

#[test]
fn without_a_config_the_defaults_and_the_daemons_model_apply() {
    let lines = resolve(&Asked::default(), &TurnDefaults::default(), &models().models).unwrap();
    assert_eq!(
        values(&lines),
        [
            ("mode", Some("cautious".to_owned()), SettingSource::Default),
            ("model", Some("gpt-5.5".to_owned()), SettingSource::DaemonDefault),
            (
                "effort",
                Some("medium".to_owned()),
                SettingSource::ModelDefault { model: "gpt-5.5".to_owned() }
            ),
        ]
    );
    assert_eq!(lines[0].choices, ["manual", "cautious", "auto"]);
    assert_eq!(lines[1].choices, ["gpt-5.5", "gpt-5.4", "my-model"]);
    assert_eq!(lines[2].choices, ["low", "medium", "high", "xhigh"]);
}

#[test]
fn the_config_file_gives_the_defaults_it_sets() {
    let defaults = defaults(Some(Mode::Manual), Some("gpt-5.5"), Some("high"));
    let lines = resolve(&Asked::default(), &defaults, &models().models).unwrap();
    assert_eq!(
        values(&lines),
        [
            ("mode", Some("manual".to_owned()), file()),
            ("model", Some("gpt-5.5".to_owned()), file()),
            ("effort", Some("high".to_owned()), file()),
        ]
    );
}

#[test]
fn what_a_prompt_asks_for_wins_over_the_config() {
    let asked = Asked {
        mode: Some(Given { value: Mode::Auto, source: SettingSource::Var(Var::Mode) }),
        model: given("gpt-5.4", SettingSource::Flag("--model")),
        effort: given("low", SettingSource::Var(Var::Effort)),
    };
    let defaults = defaults(Some(Mode::Manual), Some("gpt-5.5"), Some("high"));
    let lines = resolve(&asked, &defaults, &models().models).unwrap();
    assert_eq!(
        values(&lines),
        [
            ("mode", Some("auto".to_owned()), SettingSource::Var(Var::Mode)),
            ("model", Some("gpt-5.4".to_owned()), SettingSource::Flag("--model")),
            ("effort", Some("low".to_owned()), SettingSource::Var(Var::Effort)),
        ]
    );
    assert_eq!(lines[2].choices, ["low", "medium", "high"], "the efforts of the asked model");
}

#[test]
fn a_model_without_a_default_effort_sends_none() {
    let asked =
        Asked { model: given("gpt-5.4", SettingSource::Flag("--model")), ..Asked::default() };
    let lines = resolve(&asked, &TurnDefaults::default(), &models().models).unwrap();
    assert_eq!(lines[2].value, None);
    assert_eq!(lines[2].source, SettingSource::Backend { model: "gpt-5.4".to_owned() });
}

#[test]
fn an_unknown_model_is_an_error_with_the_choices() {
    let asked = Asked { model: given("gpt-9", SettingSource::Var(Var::Model)), ..Asked::default() };
    let error = resolve(&asked, &TurnDefaults::default(), &models().models).unwrap_err();
    assert_eq!(error.exit(), Exit::Usage);
    assert_eq!(
        run::message(&error),
        "efr: the daemon has no model \"gpt-9\" (EFR_MODEL); choose one of: gpt-5.5, gpt-5.4, \
         my-model\n"
    );
}

#[test]
fn an_effort_the_model_does_not_take_is_an_error_with_its_efforts() {
    let asked = Asked {
        model: given("gpt-5.4", SettingSource::Flag("--model")),
        effort: given("xhigh", SettingSource::Flag("--effort")),
        ..Asked::default()
    };
    let error = resolve(&asked, &TurnDefaults::default(), &models().models).unwrap_err();
    assert_eq!(error.exit(), Exit::Usage);
    assert_eq!(
        run::message(&error),
        "efr: gpt-5.4 does not take the effort \"xhigh\" (--effort); choose one of: low, \
         medium, high\n"
    );
}

#[test]
fn the_configs_effort_is_checked_against_the_asked_model_too() {
    // A running terminal may switch to a model that the config's effort does not fit.
    let asked =
        Asked { model: given("gpt-5.4", SettingSource::Flag("--model")), ..Asked::default() };
    let error =
        resolve(&asked, &defaults(None, None, Some("xhigh")), &models().models).unwrap_err();
    assert!(matches!(&error, CliError::UnknownEffort { from, .. } if *from == file()), "{error:?}");
}

#[test]
fn a_model_whose_efforts_are_unknown_takes_any_effort() {
    let asked = Asked {
        model: given("my-model", SettingSource::Flag("--model")),
        effort: given("turbo", SettingSource::Flag("--effort")),
        ..Asked::default()
    };
    let lines = resolve(&asked, &TurnDefaults::default(), &models().models).unwrap();
    assert_eq!(lines[2].value.as_deref(), Some("turbo"));
    assert!(lines[2].choices.is_empty());
}

#[test]
fn without_a_default_model_and_without_a_model_asked_for_it_fails() {
    let mut list = models().models;
    for model in &mut list {
        model.default = false;
    }
    let error = resolve(&Asked::default(), &TurnDefaults::default(), &list).unwrap_err();
    assert!(matches!(error, CliError::NoDefaultModel), "{error:?}");
}

#[test]
fn an_empty_model_list_takes_any_model_as_the_daemon_does() {
    // The daemon's list is empty for `openai-api` without `[openai] models`.
    let bare = resolve(&Asked::default(), &TurnDefaults::default(), &[]).unwrap();
    assert_eq!(
        values(&bare),
        [
            ("mode", Some("cautious".to_owned()), SettingSource::Default),
            ("model", None, SettingSource::DaemonDefault),
            (
                "effort",
                None,
                SettingSource::Backend { model: "the daemon's default model".to_owned() }
            ),
        ]
    );
    assert!(show(&bare).contains("model = (the daemon's default)  # the daemon's default\n"));

    let from_file = resolve(
        &Asked { effort: given("high", SettingSource::Flag("--effort")), ..Asked::default() },
        &defaults(Some(Mode::Auto), Some("gpt-7"), None),
        &[],
    )
    .unwrap();
    assert_eq!(
        values(&from_file),
        [
            ("mode", Some("auto".to_owned()), file()),
            ("model", Some("gpt-7".to_owned()), file()),
            ("effort", Some("high".to_owned()), SettingSource::Flag("--effort")),
        ]
    );
    assert!(from_file.iter().all(|line| line.key == "mode" || line.choices.is_empty()));

    let asked = Asked { model: given("o9", SettingSource::Flag("--model")), ..Asked::default() };
    let lines = resolve(&asked, &TurnDefaults::default(), &[]).unwrap();
    assert_eq!(lines[1].value.as_deref(), Some("o9"));
    let blank = Asked { model: given(" ", SettingSource::Flag("--model")), ..Asked::default() };
    assert!(matches!(
        resolve(&blank, &TurnDefaults::default(), &[]),
        Err(CliError::UnknownModel { .. })
    ));
}

#[test]
fn a_model_whose_efforts_are_unknown_takes_only_an_effort_word_as_the_daemon_does() {
    // `my-model` comes from `[openai] models`, so its efforts are not known.
    let asked = |effort: &str| Asked {
        model: given("my-model", SettingSource::Flag("--model")),
        effort: given(effort, SettingSource::Flag("--effort")),
        ..Asked::default()
    };
    let lines = resolve(&asked("x-high"), &TurnDefaults::default(), &models().models).unwrap();
    assert_eq!(lines[2].value.as_deref(), Some("x-high"));
    for effort in ["High", "very high", &"a".repeat(33)] {
        let error =
            resolve(&asked(effort), &TurnDefaults::default(), &models().models).unwrap_err();
        assert!(matches!(error, CliError::EffortNotAWord { .. }), "{effort:?}: {error:?}");
        assert_eq!(error.exit(), Exit::Usage);
    }
    // No list at all: the same rule.
    let none = Asked { effort: given("High", SettingSource::Flag("--effort")), ..Asked::default() };
    assert!(matches!(
        resolve(&none, &TurnDefaults::default(), &[]),
        Err(CliError::EffortNotAWord { .. })
    ));
}

#[test]
fn each_setting_is_one_line_with_its_source_and_choices() {
    let lines = resolve(
        &Asked { model: given("gpt-5.4", SettingSource::Flag("--model")), ..Asked::default() },
        &TurnDefaults::default(),
        &models().models,
    )
    .unwrap();
    insta::assert_snapshot!(show(&lines));
}

#[test]
fn text_from_the_daemon_cannot_drive_the_terminal() {
    let line = Line {
        key: "model",
        value: Some("evil\x1b[2J".to_owned()),
        source: SettingSource::DaemonDefault,
        choices: vec!["evil\x1b[2J".to_owned()],
    };
    assert!(!show(&[line]).contains('\x1b'));
}

/// A context with `vars` and the client settings of a `config.toml` holding `text`.
fn context(env: &TestEnv, vars: &[(Var, &str)], config: Option<&str>) -> Context {
    let settings = match config {
        Some(text) => Settings::parse(&env.dirs.config().join("config.toml"), text),
        None => Settings::default(),
    };
    Context { env: Env::fixed(vars.iter().copied()), settings, ..env.context() }
}

#[tokio::test]
async fn efr_settings_asks_the_daemon_for_its_models_and_prints_the_settings() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = context(
        &env,
        &[(Var::Mode, "auto"), (Var::Model, "gpt-5.4")],
        Some("[model]\neffort = \"high\"\n"),
    );
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().origin, Origin::Cli);
        conn.answer_models(&models()).await;
        conn.until_closed().await;
    };
    let line = command(&["settings", "--model", "gpt-5.5"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Success, "{}", captured.stderr());
    let config = env.dirs.config().join("config.toml");
    assert_eq!(
        captured.stdout(),
        format!(
            "mode = auto  # EFR_MODE; choices: manual, cautious, auto\n\
             model = gpt-5.5  # --model; choices: gpt-5.5, gpt-5.4, my-model\n\
             effort = high  # {}; choices: low, medium, high, xhigh\n",
            config.display()
        )
    );
    assert_eq!(captured.stderr(), "");
}

#[tokio::test]
async fn a_value_the_daemon_would_refuse_exits_with_two_and_prints_nothing() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = context(&env, &[(Var::Effort, "max")], None);
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        conn.answer_models(&models()).await;
        conn.until_closed().await;
    };
    let line = command(&["settings"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::Usage);
    assert_eq!(captured.stdout(), "");
    assert_eq!(
        captured.stderr(),
        "efr: gpt-5.5 does not take the effort \"max\" (EFR_EFFORT); choose one of: low, \
         medium, high, xhigh\n"
    );
}

#[tokio::test]
async fn an_unknown_mode_fails_before_any_connection() {
    let env = TestEnv::new();
    // No daemon listens: the mode is checked first.
    let ctx = context(&env, &[(Var::Mode, "fast")], None);
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["settings"]), &ctx, &mut out).await;
    assert_eq!(exit, Exit::Usage);
    assert!(captured.stderr().contains("choose one of: manual, cautious, auto"));
}

#[tokio::test]
async fn a_daemon_that_cannot_list_models_fails_the_command() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::ModelsList(_)));
        let body = ErrorBody {
            code: ErrorCode::Internal,
            message: "models.list is not wired yet".to_owned(),
            data: None,
        };
        conn.fail(id, body).await;
        conn.until_closed().await;
    };
    let line = command(&["settings"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    assert_eq!(exit, Exit::DaemonError);
    assert!(captured.stderr().contains("models.list is not wired yet"), "{}", captured.stderr());
}

#[tokio::test]
async fn without_a_daemon_settings_exits_with_three() {
    let env = TestEnv::new();
    let (mut out, _captured) = capture();
    let exit = run::run(&command(&["settings"]), &env.context(), &mut out).await;
    assert_eq!(exit, Exit::NotRunning);
}

#[test]
fn the_client_config_keeps_the_turn_defaults_it_sets() {
    let path = Path::new(PATH);
    let settings =
        Settings::parse(path, "[permissions]\nmode = \"auto\"\n[model]\nname = \"gpt-5.4\"\n");
    assert_eq!(settings.turn, defaults(Some(Mode::Auto), Some("gpt-5.4"), None));
    let settings = Settings::parse(path, "");
    assert_eq!(settings.turn, defaults(None, None, None), "a mode left out is no file value");
}
