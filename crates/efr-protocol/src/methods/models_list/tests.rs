use super::{EFFORT_MAX_LEN, ModelInfo, ModelSource, is_effort_word};

fn model(efforts: &[&str]) -> ModelInfo {
    ModelInfo {
        id: "gpt-5.5".to_owned(),
        efforts: efforts.iter().map(|effort| (*effort).to_owned()).collect(),
        default_effort: None,
        default: false,
        source: ModelSource::Builtin,
    }
}

#[test]
fn a_model_with_known_efforts_takes_only_those() {
    let known = model(&["low", "high"]);
    assert!(known.takes_effort("low"));
    assert!(!known.takes_effort("medium"));
}

#[test]
fn a_model_with_unknown_efforts_takes_any_effort_word() {
    let unknown = model(&[]);
    for effort in ["low", "x-high", "max_2", &"a".repeat(EFFORT_MAX_LEN)] {
        assert!(unknown.takes_effort(effort), "{effort:?}");
        assert!(is_effort_word(effort), "{effort:?}");
    }
    for effort in ["", "High", "very high", "x;y", &"a".repeat(EFFORT_MAX_LEN + 1)] {
        assert!(!unknown.takes_effort(effort), "{effort:?}");
        assert!(!is_effort_word(effort), "{effort:?}");
    }
}
