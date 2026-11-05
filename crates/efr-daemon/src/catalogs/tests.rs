use std::sync::Arc;

use efr_config::Settings;
use efr_providers::openai::{Backend, Catalog, ModelCatalog};
use pretty_assertions::assert_eq;

use super::Catalogs;
use crate::catalog::{Models, ProviderCatalog};
use crate::providers::{ANTHROPIC, API, SUBSCRIPTION};

fn openai(provider: &str, backend: Backend) -> Arc<Models> {
    let catalog = ProviderCatalog::OpenAi(ModelCatalog::new(Catalog::builtin(backend)));
    Arc::new(Models::fixed(provider, catalog))
}

fn catalogs() -> Catalogs {
    // NOTE: a list of Anthropic needs a fetch or a cache file; only the provider of
    // each catalog counts here.
    Catalogs::new(
        openai(SUBSCRIPTION, Backend::Subscription),
        vec![openai(API, Backend::Api), openai(ANTHROPIC, Backend::Api)],
    )
}

/// The providers of the listed models, each once, in their order.
fn providers(catalogs: &Catalogs, settings: &Settings) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for model in catalogs.listed(settings).models {
        let provider = model.provider.expect("each model names its provider");
        if seen.last() != Some(&provider) {
            assert!(!seen.contains(&provider), "{provider} comes twice");
            seen.push(provider);
        }
    }
    seen
}

#[test]
fn the_configured_provider_comes_first_and_others_only_with_a_login() {
    let catalogs = catalogs();
    let mut settings = Settings::default();
    settings.model.provider = ANTHROPIC.to_owned();

    // Without a login the configured provider is listed all the same.
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC]);
    assert_eq!(catalogs.listed(&settings).catalogs.len(), 1);

    catalogs.set_logged_in(API, true);
    catalogs.set_logged_in(SUBSCRIPTION, true);
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC, SUBSCRIPTION, API]);
    let listed = catalogs.listed(&settings);
    assert_eq!(listed.catalogs.len(), 3);
    let defaults: Vec<_> = listed.models.iter().filter(|model| model.default).collect();
    assert_eq!(defaults.len(), 1, "{defaults:?}");
    assert_eq!(defaults[0].provider.as_deref(), Some(ANTHROPIC));

    catalogs.set_logged_in(SUBSCRIPTION, false);
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC, API]);
}

#[test]
fn the_list_follows_the_provider_of_the_settings() {
    let catalogs = catalogs();
    catalogs.set_logged_in(ANTHROPIC, true);
    let mut settings = Settings::default();
    settings.model.provider = SUBSCRIPTION.to_owned();
    assert_eq!(providers(&catalogs, &settings), [SUBSCRIPTION, ANTHROPIC]);

    settings.model.provider = ANTHROPIC.to_owned();
    assert_eq!(providers(&catalogs, &settings), [ANTHROPIC]);
    assert_eq!(catalogs.configured(&settings).provider(), ANTHROPIC);
}

#[test]
fn the_routing_order_puts_a_provider_without_a_login_behind_the_others() {
    let catalogs = catalogs();
    let settings = Settings::default();
    assert_eq!(settings.model.provider, SUBSCRIPTION);
    catalogs.set_logged_in(API, true);

    let routed: Vec<_> =
        catalogs.routed(&settings).into_iter().map(|model| model.provider.unwrap()).collect();
    let first_subscription = routed.iter().position(|provider| provider == SUBSCRIPTION);
    assert_eq!(routed.first().map(String::as_str), Some(API), "{routed:?}");
    assert!(first_subscription.is_some(), "the configured provider stays in the list");
    assert!(routed[..first_subscription.unwrap()].iter().all(|provider| provider == API));
    // The listing keeps its own order: the configured provider first.
    let listed = catalogs.listed(&settings).models;
    assert_eq!(listed[0].provider.as_deref(), Some(SUBSCRIPTION));

    // With a login, the configured provider routes first again.
    catalogs.set_logged_in(SUBSCRIPTION, true);
    assert_eq!(
        catalogs.routed(&settings)[0].provider.as_deref(),
        Some(SUBSCRIPTION),
        "a shared id goes to the provider of new conversations"
    );
}
