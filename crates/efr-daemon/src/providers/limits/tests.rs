use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{AccountLimits, LimitKind, LimitReached, LimitWindow};
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

/// A window of `minutes` at `percent` that resets `after` from now.
fn window(clock: &TestClock, minutes: u32, percent: u8, after: Duration) -> LimitWindow {
    LimitWindow {
        minutes: Some(minutes),
        used_percent: percent,
        resets_at: Some(clock.shared().now() + after),
    }
}

#[test]
fn the_windows_of_a_plan_show_with_their_time_and_each_goes_at_its_reset() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    let short = window(&clock, 300, 92, hour);
    let long = window(&clock, 10080, 41, hour * 24);
    let learned = clock.shared().now();

    book.note_windows(PROVIDER, &[short.clone(), long.clone()]);
    clock.advance(Duration::from_secs(720));

    assert_eq!(
        book.limits(PROVIDER),
        Some(AccountLimits {
            observed_at: learned,
            windows: vec![short, long.clone()],
            reached: None,
        })
    );
    assert_eq!(book.reached(PROVIDER), None, "a window under 100 % stops nothing");
    assert_eq!(book.limits("openai-subscription"), None, "each account has its own");
    clock.advance(hour);
    assert_eq!(
        book.limits(PROVIDER).map(|limits| limits.windows),
        Some(vec![long]),
        "the short window reset"
    );
    clock.advance(hour * 24);
    assert_eq!(book.limits(PROVIDER), None, "no window left");
}

#[test]
fn a_newer_report_replaces_the_windows_and_an_empty_one_changes_nothing() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    book.note_windows(PROVIDER, &[window(&clock, 300, 40, hour)]);
    clock.advance(Duration::from_secs(60));
    let newer = window(&clock, 300, 55, hour);
    let learned = clock.shared().now();

    book.note_windows(PROVIDER, std::slice::from_ref(&newer));
    book.note_windows(PROVIDER, &[]);

    let limits = book.limits(PROVIDER).unwrap();
    assert_eq!(limits.windows, vec![newer]);
    assert_eq!(limits.observed_at, learned);
}

#[test]
fn a_full_window_stops_the_account_until_the_last_full_one_resets() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    let short = window(&clock, 300, 100, hour);
    let long = window(&clock, 10080, 100, hour * 30);
    book.note_windows(PROVIDER, &[short.clone(), long.clone()]);

    assert_eq!(
        book.reached(PROVIDER),
        Some(LimitReached { kind: LimitKind::Usage, resets_at: long.resets_at })
    );
    assert_eq!(book.limits(PROVIDER).unwrap().reached, None, "the windows show it");

    let open = LimitWindow { minutes: None, used_percent: 100, resets_at: None };
    book.note_windows(PROVIDER, &[short, open]);
    assert_eq!(
        book.reached(PROVIDER),
        Some(LimitReached { kind: LimitKind::Usage, resets_at: None }),
        "a full window with no reset ends at no known time"
    );
}

#[test]
fn a_call_that_works_keeps_the_windows_and_a_login_clears_everything() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    let short = window(&clock, 300, 92, hour);
    book.note_windows(PROVIDER, std::slice::from_ref(&short));
    book.note(PROVIDER, &usage(&clock, hour));
    clock.advance(Duration::from_secs(60));

    book.served(PROVIDER);

    assert_eq!(
        book.limits(PROVIDER),
        Some(AccountLimits {
            observed_at: clock.shared().now() - Duration::from_secs(60),
            windows: vec![short],
            reached: None,
        })
    );
    book.note(PROVIDER, &usage(&clock, hour));
    book.clear(PROVIDER);
    assert_eq!(book.limits(PROVIDER), None);
    assert_eq!(book.reached(PROVIDER), None);
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
    ProviderError::LimitReached { reached: reached.clone(), message: None, windows: Vec::new() }
}

/// Limits research 2.3: the 429 of a plan at its usage limit tells its windows, so
/// `efr status` shows `5-hour 100% reached` (TM-Accounts 8), not only the stop.
#[tokio::test]
async fn a_limit_notes_the_windows_that_its_answer_told() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    let reached = usage(&clock, hour);
    let windows = vec![window(&clock, 300, 100, hour), window(&clock, 10080, 41, hour * 24)];
    let error = ProviderError::LimitReached {
        reached: reached.clone(),
        message: None,
        windows: windows.clone(),
    };
    let inner = Arc::new(Scripted {
        id: ProviderId::new(PROVIDER).unwrap(),
        answers: Mutex::new(vec![Err(error)].into()),
    });
    let provider = Limited::new(inner, PROVIDER, Arc::clone(&book));

    assert!(call(&provider).await);
    assert_eq!(
        book.limits(PROVIDER),
        Some(AccountLimits { observed_at: clock.shared().now(), windows, reached: Some(reached) })
    );
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

#[tokio::test]
async fn a_limited_provider_notes_the_windows_and_the_empty_bucket_of_a_rate_limit() {
    let clock = TestClock::new();
    let book = book(&clock);
    let hour = Duration::from_secs(3600);
    let short = window(&clock, 300, 92, hour);
    let bucket = LimitReached {
        kind: LimitKind::Requests,
        resets_at: Some(clock.shared().now() + Duration::from_secs(180)),
    };
    let limits = || Ok(ProviderEvent::Limits { windows: vec![short.clone()] });
    let inner = Arc::new(Scripted {
        id: ProviderId::new(PROVIDER).unwrap(),
        answers: Mutex::new(
            vec![
                Ok(vec![limits(), done()]),
                Err(ProviderError::RateLimited { retry_after: None, limit: None }),
                Err(ProviderError::RateLimited { retry_after: None, limit: Some(bucket.clone()) }),
                Ok(vec![limits(), done()]),
            ]
            .into(),
        ),
    });
    let provider = Limited::new(inner, PROVIDER, Arc::clone(&book));

    let events = provider.stream(Request::new("gpt-6.1-sol")).await.unwrap();
    let events: Vec<_> = events.collect().await;
    assert!(
        matches!(events.first(), Some(Ok(ProviderEvent::Limits { .. }))),
        "the windows go on to the turn: {events:?}"
    );
    assert_eq!(book.limits(PROVIDER).unwrap().windows, vec![short.clone()]);

    assert!(call(&provider).await);
    assert_eq!(book.reached(PROVIDER), None, "too many requests with no bucket is no limit");
    assert!(call(&provider).await);
    assert_eq!(book.reached(PROVIDER), Some(bucket.clone()), "the bucket that ran out");
    assert_eq!(book.limits(PROVIDER).unwrap().reached, Some(bucket));
    assert!(!call(&provider).await);
    assert_eq!(
        book.limits(PROVIDER),
        Some(AccountLimits {
            observed_at: clock.shared().now(),
            windows: vec![short],
            reached: None
        }),
        "the key serves again"
    );
}
