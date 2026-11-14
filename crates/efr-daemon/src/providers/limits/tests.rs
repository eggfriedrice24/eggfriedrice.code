use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{AccountLimits, LimitKind, LimitReached};
use efr_provider::{
    EditTool, ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, StopReason, WebSupport,
};
use efr_test_support::TestClock;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use super::{LimitBook, Limited};

const PROVIDER: &str = "openai-subscription.b";

fn book(clock: &TestClock) -> Arc<LimitBook> {
    Arc::new(LimitBook::new(clock.shared()))
}

fn usage(clock: &TestClock, after: Duration) -> LimitReached {
    LimitReached { kind: LimitKind::Usage, resets_at: Some(clock.shared().now() + after) }
}

#[test]
fn a_limit_holds_until_its_reset_and_shows_when_efr_learned_it() {
    let clock = TestClock::new();
    let book = book(&clock);
    let reached = usage(&clock, Duration::from_secs(3600));
    let learned = clock.shared().now();

    book.note(PROVIDER, &reached);
    clock.advance(Duration::from_secs(60));

    assert_eq!(book.reached(PROVIDER), Some(reached.clone()));
    assert_eq!(
        book.limits(PROVIDER),
        Some(AccountLimits { observed_at: learned, windows: Vec::new(), reached: Some(reached) })
    );
    assert_eq!(book.reached("openai-subscription"), None, "each account has its own");
    clock.advance(Duration::from_secs(3540));
    assert_eq!(book.reached(PROVIDER), None, "the limit ends at its reset");
    assert_eq!(book.limits(PROVIDER), None);
}

#[test]
fn a_limit_without_a_reset_holds_until_it_is_cleared_and_too_many_requests_is_no_limit() {
    let clock = TestClock::new();
    let book = book(&clock);
    let credit = LimitReached { kind: LimitKind::Credit, resets_at: None };

    book.note(PROVIDER, &credit);
    clock.advance(Duration::from_secs(86_400 * 30));
    assert_eq!(book.reached(PROVIDER), Some(credit));
    book.clear(PROVIDER);
    assert_eq!(book.reached(PROVIDER), None);

    book.note(PROVIDER, &LimitReached { kind: LimitKind::TooManyRequests, resets_at: None });
    assert_eq!(book.reached(PROVIDER), None);
}

/// One answer of [`Scripted`]: the stream of a call, or the error before it.
type Answer = Result<Vec<Result<ProviderEvent, ProviderError>>, ProviderError>;

/// A provider that answers each call with the next scripted answer.
#[derive(Debug)]
struct Scripted {
    id: ProviderId,
    answers: Mutex<VecDeque<Answer>>,
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new("gpt-6.1-sol")]
    }

    fn default_edit_tool(&self) -> EditTool {
        EditTool::ApplyPatch
    }

    fn web_search(&self, _effort: Option<&str>) -> WebSupport {
        WebSupport::StoredOrLive
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let events = self.answers.lock().unwrap().pop_front().unwrap()?;
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

fn done() -> Result<ProviderEvent, ProviderError> {
    Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None })
}

fn limit(reached: &LimitReached) -> ProviderError {
    ProviderError::LimitReached { reached: reached.clone(), message: None }
}

/// Runs one call through `provider` to its end, and returns whether it failed.
async fn call(provider: &Limited) -> bool {
    match provider.stream(Request::new("gpt-6.1-sol")).await {
        Ok(stream) => stream.collect::<Vec<_>>().await.iter().any(Result::is_err),
        Err(_) => true,
    }
}

#[tokio::test]
async fn a_limited_provider_notes_a_limit_and_a_call_that_works_clears_it() {
    let clock = TestClock::new();
    let book = book(&clock);
    let reached = usage(&clock, Duration::from_secs(600));
    let inner = Arc::new(Scripted {
        id: ProviderId::new(PROVIDER).unwrap(),
        answers: Mutex::new(
            vec![
                Err(limit(&reached)),
                Ok(vec![done()]),
                Ok(vec![Err(limit(&reached))]),
                Err(ProviderError::Incomplete),
            ]
            .into(),
        ),
    });
    let provider = Limited::new(inner, PROVIDER, Arc::clone(&book));

    assert_eq!(provider.id().as_str(), PROVIDER);
    assert_eq!(provider.models().len(), 1, "the inner provider's models");
    assert_eq!(provider.default_edit_tool(), EditTool::ApplyPatch);
    assert_eq!(provider.web_search(None), WebSupport::StoredOrLive);

    assert!(call(&provider).await, "the call still fails");
    assert_eq!(book.reached(PROVIDER), Some(reached.clone()));
    assert!(!call(&provider).await);
    assert_eq!(book.reached(PROVIDER), None, "the account serves again");
    assert!(call(&provider).await, "a limit as an item of the stream");
    assert_eq!(book.reached(PROVIDER), Some(reached.clone()));
    assert!(call(&provider).await);
    assert_eq!(book.reached(PROVIDER), Some(reached), "another error changes nothing");
}
