use std::path::Path;

use efr_protocol::{
    CacheMode, EffectiveSettings, ErrorCode, Mode, ModeFallback, ModelInfo, ModelSource,
    NetworkMode, Origin, OverriddenSettings, SandboxStatus, TurnSettings,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Place, failure, resolve};
use crate::{ConversationConfig, ConversationError};

const HOME: &str = "/home/u";

/// A sandbox that the probe found ready.
fn ready() -> SandboxStatus {
    SandboxStatus {
        available: true,
        reason: None,
        fix: None,
        landlock_abi: Some(10),
        errata: Some(0xf),
        bwrap: Some("/usr/bin/bwrap".into()),
        bwrap_version: Some("0.13.0".to_owned()),
        cache_mode: CacheMode::Tmp,
        network_mode: NetworkMode::None,
        warnings: Vec::new(),
    }
}

/// A turn outside any registered project, with `sandbox`.
fn place(sandbox: &SandboxStatus) -> Place<'_> {
    Place { sandbox, project_root: None, home: Path::new(HOME) }
}

fn model(id: &str, efforts: &[&str], default_effort: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: Some(default_effort.to_owned()),
        default: id == "gpt-5.5",
        source: ModelSource::Builtin,
        context_window: None,
    }
}

fn config() -> ConversationConfig {
    let mut config = ConversationConfig::new("gpt-5.5", "/scratch");
    config.models = vec![
        model("gpt-6-sol", &["low", "medium", "high", "xhigh", "max", "ultra"], "medium"),
        model("gpt-5.5", &["low", "medium", "high", "xhigh"], "medium"),
    ];
    config
}

fn asked(mode: Option<Mode>, model: Option<&str>, effort: Option<&str>) -> TurnSettings {
    TurnSettings { mode, model: model.map(str::to_owned), effort: effort.map(str::to_owned) }
}

#[test]
fn a_prompt_without_settings_runs_with_the_config_defaults() {
    let mut config = config();
    config.effort = Some("high".to_owned());
    config.mode = Mode::Manual;

    let settings =
        resolve(&TurnSettings::default(), &config, Origin::Shell, place(&ready())).unwrap();

    assert_eq!(
        settings,
        EffectiveSettings {
            mode: Mode::Manual,
            model: "gpt-5.5".to_owned(),
            effort: Some("high".to_owned()),
            overridden: OverriddenSettings::default(),
            fallback: None,
        }
    );
}

#[test]
fn a_prompts_own_values_win_and_are_marked() {
    let settings = resolve(
        &asked(Some(Mode::Auto), Some("gpt-6-sol"), Some("ultra")),
        &config(),
        Origin::Cli,
        place(&ready()),
    )
    .unwrap();

    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(settings.model, "gpt-6-sol");
    assert_eq!(settings.effort.as_deref(), Some("ultra"));
    assert_eq!(settings.overridden, OverriddenSettings { mode: true, model: true, effort: true });
}

#[test]
fn no_effort_leaves_it_to_the_backend() {
    let settings =
        resolve(&asked(None, Some("gpt-6-sol"), None), &config(), Origin::Shell, place(&ready()))
            .unwrap();
    assert_eq!(settings.effort, None);
    assert_eq!(settings.overridden, OverriddenSettings { model: true, ..Default::default() });
}

#[test]
fn a_remote_turn_runs_with_at_most_cautious() {
    let mut config = config();
    config.mode = Mode::Auto;
    let from_config =
        resolve(&TurnSettings::default(), &config, Origin::Phone, place(&ready())).unwrap();
    let asked_auto =
        resolve(&asked(Some(Mode::Auto), None, None), &config, Origin::Phone, place(&ready()))
            .unwrap();
    let asked_manual =
        resolve(&asked(Some(Mode::Manual), None, None), &config, Origin::Phone, place(&ready()))
            .unwrap();
    let local = resolve(&TurnSettings::default(), &config, Origin::Proxy, place(&ready())).unwrap();

    assert_eq!(from_config.mode, Mode::Cautious);
    assert_eq!(asked_auto.mode, Mode::Cautious);
    assert_eq!(asked_manual.mode, Mode::Manual, "a stricter mode stays");
    assert_eq!(local.mode, Mode::Auto);
}

#[test]
fn a_model_outside_the_list_fails_with_the_choices() {
    let error =
        resolve(&asked(None, Some("gpt-4o"), None), &config(), Origin::Shell, place(&ready()))
            .unwrap_err();

    assert!(
        matches!(
            &error,
            ConversationError::InvalidSetting { setting: "model", value, model: None, choices, from_config: false }
                if value == "gpt-4o" && *choices == ["gpt-6-sol", "gpt-5.5"]
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the model gpt-4o is not in the model list; choose one of: gpt-6-sol, gpt-5.5"
    );
}

#[test]
fn an_effort_the_model_does_not_take_fails_with_its_efforts() {
    let error =
        resolve(&asked(None, None, Some("ultra")), &config(), Origin::Shell, place(&ready()))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "the effort ultra is not an effort of gpt-5.5; choose one of: low, medium, high, xhigh"
    );
    let body = failure(&error);
    assert_eq!(body.code, ErrorCode::Invalid);
    assert_eq!(
        body.data,
        Some(json!({
            "setting": "effort",
            "value": "ultra",
            "model": "gpt-5.5",
            "choices": ["low", "medium", "high", "xhigh"],
        }))
    );
}

#[test]
fn a_config_default_that_does_not_fit_the_prompts_model_says_so() {
    let mut config = config();
    config.effort = Some("ultra".to_owned());
    let fits =
        resolve(&asked(None, Some("gpt-6-sol"), None), &config, Origin::Shell, place(&ready()));
    let error =
        resolve(&TurnSettings::default(), &config, Origin::Shell, place(&ready())).unwrap_err();

    assert_eq!(fits.unwrap().effort.as_deref(), Some("ultra"));
    assert!(
        error.to_string().starts_with("the config's default effort ultra is not an effort of"),
        "{error}"
    );
    assert!(matches!(error, ConversationError::InvalidSetting { from_config: true, .. }));
}

#[test]
fn without_a_model_list_any_model_and_any_effort_word_pass() {
    let config = ConversationConfig::new("gpt-4.1", "/scratch");

    let settings = resolve(
        &asked(None, Some("o3-mini"), Some("minimal")),
        &config,
        Origin::Shell,
        place(&ready()),
    )
    .unwrap();
    let empty = resolve(&asked(None, Some(" "), None), &config, Origin::Shell, place(&ready()));
    let odd =
        resolve(&asked(None, None, Some("High Effort")), &config, Origin::Shell, place(&ready()));

    assert_eq!(settings.model, "o3-mini");
    assert_eq!(settings.effort.as_deref(), Some("minimal"));
    assert!(matches!(empty, Err(ConversationError::InvalidSetting { setting: "model", .. })));
    assert!(matches!(
        odd,
        Err(ConversationError::InvalidSetting { setting: "effort", ref choices, .. }) if choices.is_empty()
    ));
}

#[test]
fn a_model_whose_efforts_are_not_known_takes_any_effort_word() {
    let mut config = config();
    config.models.push(ModelInfo {
        id: "gpt-next".to_owned(),
        efforts: Vec::new(),
        default_effort: None,
        default: false,
        source: ModelSource::Config,
        context_window: None,
    });

    let settings = resolve(
        &asked(None, Some("gpt-next"), Some("turbo")),
        &config,
        Origin::Shell,
        place(&ready()),
    )
    .unwrap();

    assert_eq!(settings.effort.as_deref(), Some("turbo"));
}

#[test]
fn auto_falls_back_to_cautious_with_the_probes_reason() {
    let unavailable = SandboxStatus::unavailable("Landlock ABI 6 found; auto needs 9 (Linux 7.1)");

    let settings = resolve(
        &asked(Some(Mode::Auto), None, None),
        &config(),
        Origin::Shell,
        place(&unavailable),
    )
    .unwrap();

    assert_eq!(settings.mode, Mode::Cautious);
    assert!(settings.overridden.mode, "the prompt still asked for a mode");
    assert_eq!(
        settings.fallback,
        Some(ModeFallback {
            asked: Mode::Auto,
            reason: "Landlock ABI 6 found; auto needs 9 (Linux 7.1)".to_owned(),
        })
    );
}

#[test]
fn an_unavailable_sandbox_without_a_reason_still_names_one() {
    let mut unavailable = SandboxStatus::unavailable("");
    unavailable.reason = None;
    let mut config = config();
    config.mode = Mode::Auto;

    let settings =
        resolve(&TurnSettings::default(), &config, Origin::Shell, place(&unavailable)).unwrap();

    assert_eq!(settings.mode, Mode::Cautious);
    assert_eq!(
        settings.fallback.map(|fallback| fallback.reason),
        Some(ModeFallback::UNAVAILABLE_REASON.to_owned())
    );
}

#[test]
fn a_project_at_home_or_above_runs_auto_as_cautious() {
    let ready = ready();
    for root in [HOME, "/home", "/"] {
        let place = Place { project_root: Some(Path::new(root)), ..place(&ready) };
        let settings =
            resolve(&asked(Some(Mode::Auto), None, None), &config(), Origin::Shell, place).unwrap();
        assert_eq!(settings.mode, Mode::Cautious, "{root}");
        assert_eq!(
            settings.fallback.map(|fallback| fallback.reason),
            Some(ModeFallback::HOME_PROJECT_REASON.to_owned()),
            "{root}"
        );
    }
    let project = Place { project_root: Some(Path::new("/home/u/dotfiles")), ..place(&ready) };
    let settings =
        resolve(&asked(Some(Mode::Auto), None, None), &config(), Origin::Shell, project).unwrap();
    assert_eq!((settings.mode, settings.fallback), (Mode::Auto, None));
}

#[test]
fn only_auto_needs_the_sandbox() {
    let unavailable = SandboxStatus::unavailable("bubblewrap is not installed");
    for mode in [Mode::Manual, Mode::Cautious] {
        let settings =
            resolve(&asked(Some(mode), None, None), &config(), Origin::Shell, place(&unavailable))
                .unwrap();
        assert_eq!((settings.mode, settings.fallback), (mode, None));
    }
    // A phone turn is capped at cautious before the sandbox matters, so it falls back
    // from nothing.
    let phone = resolve(
        &asked(Some(Mode::Auto), None, None),
        &config(),
        Origin::Phone,
        place(&unavailable),
    )
    .unwrap();
    assert_eq!((phone.mode, phone.fallback), (Mode::Cautious, None));
}
