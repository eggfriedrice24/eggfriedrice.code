use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{ModelInfo, ModelSource};
use efr_provider::{Provider, ProviderError, ProviderId, ProviderStream, Request};

use super::{OneProvider, ProviderRouter, provider_after, provider_id, provider_of};
use crate::{ConversationConfig, ConversationError};

#[derive(Debug)]
struct Named(ProviderId);

#[async_trait]
impl Provider for Named {
    fn id(&self) -> &ProviderId {
        &self.0
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        unreachable!("the router tests send no request")
    }
}

/// A router of `ids`, with the first one as its default.
#[derive(Debug)]
struct Router(Vec<Arc<dyn Provider>>);

impl Router {
    fn of(ids: &[&str]) -> Self {
        Router(
            ids.iter()
                .map(|id| Arc::new(Named(ProviderId::new(*id).unwrap())) as Arc<dyn Provider>)
                .collect(),
        )
    }
}

impl ProviderRouter for Router {
    fn default_provider(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.0[0])
    }

    fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.0.iter().find(|provider| provider.id().as_str() == id).cloned()
    }
}

fn model(id: &str, provider: Option<&str>) -> ModelInfo {
    ModelInfo {
        id: id.to_owned(),
        provider: provider.map(str::to_owned),
        efforts: Vec::new(),
        default_effort: None,
        default: false,
        source: ModelSource::Builtin,
        context_window: None,
        max_context_window: None,
        prefer_websockets: false,
    }
}

fn config(models: Vec<ModelInfo>) -> ConversationConfig {
    let mut config = ConversationConfig::new("gpt-5.5", "/scratch");
    config.models = models;
    config
}

#[test]
fn a_model_goes_to_the_first_provider_that_lists_it() {
    let router = Router::of(&["openai-subscription", "openai-api", "anthropic-api"]);
    let config = config(vec![
        model("gpt-5.5", Some("openai-subscription")),
        model("gpt-5.5", Some("openai-api")),
        model("gpt-4.1", Some("openai-api")),
        model("claude-opus-5-5", Some("anthropic-api")),
    ]);

    assert_eq!(provider_id("gpt-5.5", &config, &router), "openai-subscription");
    assert_eq!(provider_id("gpt-4.1", &config, &router), "openai-api");
    assert_eq!(provider_id("claude-opus-5-5", &config, &router), "anthropic-api");
    let provider = provider_of("claude-opus-5-5", &config, &router).unwrap();
    assert_eq!(provider.id().as_str(), "anthropic-api");
}

#[test]
fn a_model_that_no_list_names_goes_to_the_default_provider() {
    let router = Router::of(&["anthropic-api", "openai-api"]);
    let config = config(vec![model("gpt-5.5", Some("openai-api")), model("o9", None)]);

    assert_eq!(provider_id("claude-next", &config, &router), "anthropic-api");
    // A list entry that names no provider counts as no entry.
    assert_eq!(provider_id("o9", &config, &router), "anthropic-api");
    assert_eq!(
        provider_id("claude-next", &ConversationConfig::new("x", "/s"), &router),
        "anthropic-api"
    );
}

#[test]
fn a_provider_that_the_daemon_does_not_run_is_an_error() {
    let router = Router::of(&["openai-subscription"]);
    let config = config(vec![model("claude-opus-5-5", Some("anthropic-api"))]);

    let error = provider_of("claude-opus-5-5", &config, &router).unwrap_err();
    assert!(
        matches!(&error, ConversationError::NoProvider { provider } if provider == "anthropic-api"),
        "{error:?}"
    );
}

#[test]
fn one_provider_serves_every_id() {
    let named: Arc<dyn Provider> = Arc::new(Named(ProviderId::new("replay").unwrap()));
    let router = OneProvider::new(named);
    let config = config(vec![model("claude-opus-5-5", Some("anthropic-api"))]);

    assert_eq!(router.default_provider().id().as_str(), "replay");
    let provider = provider_of("claude-opus-5-5", &config, &router).unwrap();
    assert_eq!(provider.id().as_str(), "replay");
}

#[test]
fn a_compaction_after_a_turn_keeps_the_provider_of_the_turn_while_it_lists_the_model() {
    let router = Router::of(&["openai-subscription", "openai-api"]);
    let config = config(vec![
        model("gpt-5.5", Some("openai-subscription")),
        model("gpt-5.5", Some("openai-api")),
        model("gpt-4.1", Some("openai-subscription")),
    ]);
    let after = |model: &str, newest: Option<&str>| {
        provider_after(model, newest, &config, &router).unwrap().id().as_str().to_owned()
    };

    assert_eq!(after("gpt-5.5", Some("openai-api")), "openai-api");
    assert_eq!(after("gpt-5.5", None), "openai-subscription");
    // The newest turn's provider no longer lists the model.
    assert_eq!(after("gpt-4.1", Some("openai-api")), "openai-subscription");
    // The newest turn's provider no longer runs.
    assert_eq!(after("gpt-5.5", Some("anthropic-api")), "openai-subscription");
}
