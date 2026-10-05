use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{EffectiveSettings, Mode, OverriddenSettings, ProtocolError, TurnSettings};

#[test]
fn modes_are_snake_case_names_on_the_wire() {
    let names: Vec<_> = Mode::ALL.iter().map(|mode| serde_json::to_value(mode).unwrap()).collect();
    assert_eq!(names, [json!("manual"), json!("cautious"), json!("auto")]);
    for mode in Mode::ALL {
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(mode.as_str()));
        assert_eq!(mode.to_string(), mode.as_str());
    }
}

#[test]
fn cautious_is_the_default_mode() {
    assert_eq!(Mode::default(), Mode::Cautious);
}

#[test]
fn modes_are_ordered_from_the_strictest_so_min_caps_a_remote_turn() {
    assert!(Mode::Manual < Mode::Cautious && Mode::Cautious < Mode::Auto);
    assert_eq!(Mode::Auto.min(Mode::Cautious), Mode::Cautious);
    assert_eq!(Mode::Manual.min(Mode::Cautious), Mode::Manual);
}

#[test]
fn a_mode_parses_from_its_wire_name_only() {
    for mode in Mode::ALL {
        assert_eq!(mode.as_str().parse::<Mode>().unwrap(), mode);
    }
    for wrong in ["Auto", "", "yolo", " auto"] {
        let err = wrong.parse::<Mode>().unwrap_err();
        assert!(matches!(&err, ProtocolError::UnknownMode { value } if value == wrong), "{err:?}");
        assert_eq!(err.to_string(), format!("{wrong:?} is not a permission mode"));
    }
}

#[test]
fn an_unknown_mode_on_the_wire_is_an_error() {
    assert!(serde_json::from_value::<Mode>(json!("yolo")).is_err());
}

#[test]
fn turn_settings_leave_out_what_the_prompt_does_not_ask_for() {
    assert!(TurnSettings::default().is_empty());
    assert_eq!(serde_json::to_value(TurnSettings::default()).unwrap(), json!({}));
    let effort = TurnSettings { effort: Some("high".to_owned()), ..TurnSettings::default() };
    assert!(!effort.is_empty());
    assert_eq!(serde_json::to_value(&effort).unwrap(), json!({ "effort": "high" }));
    let back: TurnSettings = serde_json::from_value(json!({ "effort": "high" })).unwrap();
    assert_eq!(back, effort);
}

#[test]
fn an_effort_is_any_string_so_a_new_backend_effort_needs_no_protocol_change() {
    let settings: TurnSettings = serde_json::from_value(json!({ "effort": "ultra" })).unwrap();
    assert_eq!(settings.effort.as_deref(), Some("ultra"));
}

#[test]
fn every_turn_setting_on_its_own_makes_the_settings_non_empty() {
    let mode = TurnSettings { mode: Some(Mode::Manual), ..TurnSettings::default() };
    let model = TurnSettings { model: Some("gpt-5.4".to_owned()), ..TurnSettings::default() };
    assert!(!mode.is_empty());
    assert!(!model.is_empty());
}

#[test]
fn effective_settings_leave_out_an_absent_effort_and_unset_flags() {
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "gpt-5.5".to_owned(),
        effort: None,
        overridden: OverriddenSettings::default(),
    };
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value, json!({ "mode": "cautious", "model": "gpt-5.5" }));
    let back: EffectiveSettings = serde_json::from_value(value).unwrap();
    assert_eq!(back, settings);
}

#[test]
fn only_the_overridden_flags_that_are_set_are_written() {
    let overridden = OverriddenSettings { mode: true, model: false, effort: true };
    assert!(!overridden.is_empty());
    assert!(OverriddenSettings::default().is_empty());
    assert_eq!(serde_json::to_value(overridden).unwrap(), json!({ "mode": true, "effort": true }));
    for one in [
        OverriddenSettings { mode: true, ..OverriddenSettings::default() },
        OverriddenSettings { model: true, ..OverriddenSettings::default() },
        OverriddenSettings { effort: true, ..OverriddenSettings::default() },
    ] {
        assert!(!one.is_empty(), "{one:?}");
    }
}

#[test]
fn effective_settings_need_a_mode_and_a_model() {
    assert!(serde_json::from_value::<EffectiveSettings>(json!({ "model": "gpt-5.5" })).is_err());
    assert!(serde_json::from_value::<EffectiveSettings>(json!({ "mode": "auto" })).is_err());
}
