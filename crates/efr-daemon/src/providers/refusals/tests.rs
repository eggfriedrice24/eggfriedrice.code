use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_provider::{
    EditTool, ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, StopReason,
};
use efr_test_support::TestClock;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use super::{KeyRefusals, Watched};

const PROVIDER: &str = "anthropic-api";

fn refusals(clock: &TestClock) -> Arc<KeyRefusals> {
    Arc::new(KeyRefusals::new(clock.shared()))
}

fn unauthorized() -> ProviderError {
    ProviderError::Unauthorized { message: Some("invalid x-api-key".to_owned()) }
}

#[test]
fn a_refusal_keeps_its_time_until_a_request_with_the_key_works() {
    let clock = TestClock::new();
    let refusals = refusals(&clock);
    assert_eq!(refusals.refused_at(PROVIDER), None);

    refusals.start(PROVIDER).failed(&unauthorized());
    let first = clock.shared().now();
    clock.advance(Duration::from_secs(60));
    refusals.start(PROVIDER).failed(&unauthorized());

    assert_eq!(
        refusals.refused_at(PROVIDER),
        Some(first + Duration::from_secs(60)),
        "the time of the last refusal"
    );
    assert_eq!(refusals.refused_at("openai-api"), None, "each provider has its own key");

    refusals.start(PROVIDER).failed(&ProviderError::NotLoggedIn);
    refusals.start(PROVIDER).failed(&ProviderError::RateLimited { retry_after: None });
    assert!(refusals.refused_at(PROVIDER).is_some(), "other errors say nothing about the key");

    refusals.start(PROVIDER).worked();
    assert_eq!(refusals.refused_at(PROVIDER), None);
}

#[test]
fn a_login_forgets_the_refusal_and_an_answer_to_the_old_key_does_not_mark_the_new_one() {
    let clock = TestClock::new();
    let refusals = refusals(&clock);
    refusals.start(PROVIDER).failed(&unauthorized());
    let before_login = refusals.start(PROVIDER);

    refusals.reset(PROVIDER);

    assert_eq!(refusals.refused_at(PROVIDER), None);
    before_login.failed(&unauthorized());
    assert_eq!(refusals.refused_at(PROVIDER), None, "the old key's answer came late");
    refusals.start(PROVIDER).failed(&unauthorized());
    assert!(refusals.refused_at(PROVIDER).is_some(), "the new key can be refused too");
}

/// One answer of [`Scripted`]: the stream of a call, or the error before it.
type Answer = Result<Vec<Result<ProviderEvent, ProviderError>>, ProviderError>;

/// A provider that answers each call with the next scripted answer.
#[derive(Debug)]
struct Scripted {
    id: ProviderId,
    answers: Mutex<VecDeque<Answer>>,
}

impl Scripted {
    fn new(answers: Vec<Answer>) -> Arc<Self> {
        Arc::new(Scripted {
            id: ProviderId::new(PROVIDER).unwrap(),
            answers: Mutex::new(answers.into()),
        })
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new("claude-opus-5-5").with_edit_tool(EditTool::Replace)]
    }

    fn default_edit_tool(&self) -> EditTool {
        EditTool::Replace
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = self.answers.lock().unwrap().pop_front().unwrap()?;
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

fn done() -> Result<ProviderEvent, ProviderError> {
    Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None })
}

/// Runs one call through `provider` to its end, and returns whether it failed.
async fn call(provider: &Watched) -> bool {
    match provider.stream(Request::new("claude-opus-5-5")).await {
        Ok(stream) => stream.collect::<Vec<_>>().await.iter().any(Result::is_err),
        Err(_) => true,
    }
}

#[tokio::test]
async fn a_watched_provider_reports_each_call_and_passes_the_answer_on() {
    let clock = TestClock::new();
    let refusals = refusals(&clock);
    let inner = Scripted::new(vec![
        Err(unauthorized()),
        Ok(vec![done()]),
        Ok(vec![Err(unauthorized())]),
        Err(ProviderError::Incomplete),
        Ok(vec![Ok(ProviderEvent::TextDelta { text: "hi".to_owned() }), done()]),
    ]);
    let provider = Watched::new(inner, PROVIDER, Arc::clone(&refusals));

    assert_eq!(provider.id().as_str(), PROVIDER);
    assert_eq!(provider.models().len(), 1, "the inner provider's models");
    assert_eq!(provider.default_edit_tool(), EditTool::Replace);

    assert!(call(&provider).await, "a refused call still fails");
    let refused = refusals.refused_at(PROVIDER);
    assert_eq!(refused, Some(clock.shared().now()));

    assert!(!call(&provider).await);
    assert_eq!(refusals.refused_at(PROVIDER), None, "the key worked");

    assert!(call(&provider).await, "a 401 as the first item of the stream");
    assert!(refusals.refused_at(PROVIDER).is_some());

    assert!(call(&provider).await);
    assert!(refusals.refused_at(PROVIDER).is_some(), "an error that is not a 401 changes nothing");

    assert!(!call(&provider).await);
    assert_eq!(refusals.refused_at(PROVIDER), None);
}
