use efr_protocol::{ModelInfo, ModelSource};
use pretty_assertions::assert_eq;

use super::{family, mark};

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        provider: None,
        efforts: Vec::new(),
        default_effort: None,
        default: false,
        source: ModelSource::Builtin,
        context_window: None,
        max_context_window: None,
        prefer_websockets: false,
        legacy: false,
    }
}

/// The ids of `ids` that [`mark`] marks legacy, in their order.
fn legacy(ids: &[&str]) -> Vec<String> {
    let mut models: Vec<ModelInfo> = ids.iter().map(|id| model(id)).collect();
    mark(&mut models);
    models.into_iter().filter(|model| model.legacy).map(|model| model.id).collect()
}

#[test]
fn the_claude_list_keeps_the_newest_of_each_family_and_fable_5_1() {
    let ids = [
        "claude-opus-5-5",
        "claude-opus-5",
        "claude-fable-5-2",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
        "claude-sonnet-5",
        "claude-haiku-5-5",
        "claude-haiku-4-5",
    ];
    assert_eq!(legacy(&ids), ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"]);
}

#[test]
fn the_openai_list_keeps_the_newest_of_each_family() {
    let ids = [
        "gpt-6.1-sol",
        "gpt-6-sol",
        "gpt-6-astra",
        "gpt-5.6-astra",
        "gpt-5.6-terra",
        "gpt-5.5-terra",
        "gpt-6-luna",
    ];
    assert_eq!(legacy(&ids), ["gpt-6-sol", "gpt-5.6-astra", "gpt-5.5-terra"]);
}

#[test]
fn a_model_with_no_newer_one_in_the_same_list_is_not_legacy() {
    assert_eq!(legacy(&["gpt-6-sol", "gpt-6-luna", "claude-opus-5"]), Vec::<String>::new());
    // Each list stands alone: the newer model of another account does not count.
    let mut one = vec![model("gpt-6-sol")];
    mark(&mut one);
    assert!(!one[0].legacy);
}

#[test]
fn an_id_with_no_version_is_never_legacy() {
    assert_eq!(legacy(&["o3", "o3-mini", "o4-mini", "gpt-4o", "gpt-next"]), Vec::<String>::new());
}

#[test]
fn a_mark_is_cleared_when_the_newer_model_leaves_the_list() {
    let mut models = vec![model("gpt-6-sol")];
    models[0].legacy = true;
    mark(&mut models);
    assert!(!models[0].legacy);
}

/// A family and its version, as a test writes it.
type Family = Option<(&'static str, &'static [u32])>;

#[test]
fn families_and_versions() {
    let cases: [(&str, Family); 9] = [
        ("claude-opus-5-5", Some(("claude-opus-*", &[5, 5]))),
        ("claude-opus-5", Some(("claude-opus-*", &[5]))),
        ("claude-opus-4-1-20250805", Some(("claude-opus-*", &[4, 1]))),
        ("claude-3-5-sonnet-20241022", Some(("claude-*-sonnet", &[3, 5]))),
        ("gpt-6.1-sol", Some(("gpt-*-sol", &[6, 1]))),
        ("gpt-5.5", Some(("gpt-*", &[5, 5]))),
        ("gpt-5.1-codex-max", Some(("gpt-*-codex-max", &[5, 1]))),
        ("gpt-4o", None),
        ("o3", None),
    ];
    for (id, expected) in cases {
        let expected = expected.map(|(name, version)| (name.to_owned(), version.to_vec()));
        assert_eq!(family(id), expected, "{id}");
    }
}

#[test]
fn a_version_compares_number_by_number() {
    assert_eq!(legacy(&["gpt-5.10-sol", "gpt-5.9-sol"]), ["gpt-5.9-sol"]);
    assert_eq!(legacy(&["gpt-6-sol", "gpt-5.6-sol"]), ["gpt-5.6-sol"]);
    assert_eq!(legacy(&["claude-opus-5-5", "claude-opus-5-5-20260101"]), Vec::<String>::new());
}
