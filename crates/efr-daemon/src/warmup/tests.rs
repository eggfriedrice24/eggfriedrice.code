use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_provider::{Provider, ProviderError, ProviderId, ProviderStream, Request};
use efr_test_support::TestClock;
use futures::StreamExt as _;

use super::{HTTP_WARM_GAP, Noted, Warmups};

#[test]
fn an_http_warm_up_fetches_at_most_once_a_minute_for_each_provider() {
    let clock = TestClock::new();
    let warmups = Warmups::new(clock.shared());

    assert!(warmups.http_due("openai-api"));
    assert!(!warmups.http_due("openai-api"));
    assert!(warmups.http_due("anthropic-api"));
    clock.advance(HTTP_WARM_GAP - Duration::from_secs(1));
    assert!(!warmups.http_due("openai-api"));
    clock.advance(Duration::from_secs(1));
    assert!(warmups.http_due("openai-api"));
}

#[test]
fn an_http_warm_up_does_not_fetch_within_a_minute_of_a_prompt_of_its_provider() {
    let clock = TestClock::new();
    let warmups = Warmups::new(clock.shared());

    warmups.used("anthropic-api");
    clock.advance(Duration::from_secs(59));
    assert!(!warmups.http_due("anthropic-api"));
    assert!(warmups.http_due("openai-api"));
    clock.advance(Duration::from_secs(1));
    assert!(warmups.http_due("anthropic-api"));
}

/// A provider whose answer ends with no event.
#[derive(Debug)]
struct Quiet(ProviderId);

#[async_trait]
impl Provider for Quiet {
    fn id(&self) -> &ProviderId {
        &self.0
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[tokio::test]
async fn a_model_call_is_a_use_of_its_provider_when_it_starts_and_when_its_answer_ends() {
    let clock = TestClock::new();
    let warmups = Arc::new(Warmups::new(clock.shared()));
    let inner = Arc::new(Quiet(ProviderId::new("anthropic-api").unwrap()));
    let provider = Noted::new(inner, "anthropic-api", Arc::clone(&warmups));

    let answer = provider.stream(Request::new("claude-opus-5-5")).await.unwrap();
    clock.advance(Duration::from_secs(59));
    assert!(!warmups.http_due("anthropic-api"), "the call started a moment ago");
    // A long answer: the turn's connection is fresh when it ends.
    clock.advance(HTTP_WARM_GAP * 3);
    let _ = answer.collect::<Vec<_>>().await;
    clock.advance(Duration::from_secs(5));

    assert!(!warmups.http_due("anthropic-api"), "the answer ended a moment ago");
    assert!(warmups.http_due("openai-api"));
}
