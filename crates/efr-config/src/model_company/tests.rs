use pretty_assertions::assert_eq;

use super::{ForeignModel, ModelCompany};
use crate::Settings;

fn settings(provider: &str, name: Option<&str>) -> Settings {
    let mut settings = Settings::default();
    settings.model.provider = provider.to_owned();
    settings.model.name = name.map(str::to_owned);
    settings
}

#[test]
fn the_form_of_an_id_names_its_company() {
    for id in ["claude-opus-5-5", "claude-haiku-5-5"] {
        assert_eq!(ModelCompany::of_model(id), Some(ModelCompany::Anthropic), "{id}");
    }
    for id in ["gpt-5.5", "gpt-6-luna", "chatgpt-4o-latest", "codex-mini", "o3", "o4-mini", "o1"] {
        assert_eq!(ModelCompany::of_model(id), Some(ModelCompany::OpenAi), "{id}");
    }
    for id in ["my-model", "opus", "o", "o3x", "oz-1", "llama-4", "claude", "gpt"] {
        assert_eq!(ModelCompany::of_model(id), None, "{id}");
    }
}

#[test]
fn each_provider_has_its_company() {
    assert_eq!(ModelCompany::of_provider("openai-subscription"), Some(ModelCompany::OpenAi));
    assert_eq!(ModelCompany::of_provider("openai-api"), Some(ModelCompany::OpenAi));
    assert_eq!(ModelCompany::of_provider("anthropic-api"), Some(ModelCompany::Anthropic));
    assert_eq!(ModelCompany::of_provider("gemini-api"), None);
}

#[test]
fn a_model_of_the_other_company_is_foreign_with_the_cause_and_the_fix() {
    let claude = settings("anthropic-api", Some("gpt-5.5"));
    let foreign = claude.foreign_model().unwrap();

    assert_eq!(
        foreign,
        ForeignModel { name: "gpt-5.5", company: ModelCompany::OpenAi, provider: "anthropic-api" }
    );
    assert_eq!(
        foreign.to_string(),
        "[model] name gpt-5.5 is a model of OpenAI, but [model] provider is anthropic-api"
    );
    assert_eq!(foreign.fix(), "set [model] name to a model of anthropic-api, or remove it");

    let openai = settings("openai-subscription", Some("claude-opus-5-5"));
    let foreign = openai.foreign_model().unwrap();
    assert_eq!(foreign.company, ModelCompany::Anthropic);
    assert_eq!(foreign.provider, "openai-subscription");
}

#[test]
fn a_model_of_the_own_company_an_unknown_form_or_no_name_is_not_foreign() {
    assert_eq!(settings("anthropic-api", Some("claude-opus-5-5")).foreign_model(), None);
    assert_eq!(settings("openai-api", Some("gpt-5.5")).foreign_model(), None);
    assert_eq!(settings("anthropic-api", Some("my-proxy-model")).foreign_model(), None);
    assert_eq!(settings("anthropic-api", None).foreign_model(), None);
    assert_eq!(Settings::default().foreign_model(), None);
}

#[test]
fn a_model_in_the_list_of_the_providers_company_is_not_foreign() {
    let mut claude = settings("anthropic-api", Some("gpt-5.5"));
    claude.anthropic.models = Some(vec!["gpt-5.5".into()]);
    assert_eq!(claude.foreign_model(), None, "the user says that the provider serves it");

    let mut openai = settings("openai-api", Some("claude-opus-5-5"));
    openai.anthropic.models = Some(vec!["claude-opus-5-5".into()]);
    assert!(openai.foreign_model().is_some(), "the list of the other company does not count");
}
