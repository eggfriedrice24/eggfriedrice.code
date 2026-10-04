//! A retry policy that waits on the injected clock.
//!
//! [`RetryPolicy::decide`] is a pure function of the attempt number, what the attempt
//! said ([`Outcome`]) and a random draw, so its table is tested without IO.
//! [`RetryPolicy::run`] loops over an operation and sleeps through
//! [`Clock::sleep`], so a test drives it with a manual clock and never waits.
//!
//! Only failures before a response body is read are retried: the operation returns a
//! response or an error, and the loop never sees a half-read body.
//!
//! A request that is not idempotent, such as a `POST` to the Responses API, is sent
//! again only when the server certainly did not act on it: no connection could be made,
//! or the server answered 408, 429 or 503, which say that it did not handle the
//! request. After a timeout, a broken connection or another 5xx the server may have
//! acted, and a second copy could run the work twice.

use std::future::Future;
use std::time::Duration;

use http::{HeaderMap, StatusCode};
use jiff::Timestamp;
use jiff::fmt::rfc2822::DateTimeParser;

use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::{HttpError, HttpResponse};

/// The non-standard header in which OpenAI and others give the wait in milliseconds.
const RETRY_AFTER_MS: &str = "retry-after-ms";

static HTTP_DATE: DateTimeParser = DateTimeParser::new();

/// When and how often to try a request again.
///
/// Build one with [`RetryPolicy::default`] or [`RetryPolicy::none`] and adjust the
/// fields; the struct is `#[non_exhaustive]` so that a later field does not break
/// callers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetryPolicy {
    /// Attempts in total, the first included. `1` never retries.
    pub max_attempts: u32,
    /// The wait before the first retry. Each later retry doubles it.
    pub initial_backoff: Duration,
    /// The longest wait the doubling reaches.
    pub max_backoff: Duration,
    /// The longest wait a server may ask for with `Retry-After`. A longer request ends
    /// the loop instead, since the caller is better placed to wait that long.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    /// Four attempts, waits of about 0.5 s, 1 s and 2 s, and server-requested waits up
    /// to one minute.
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 4,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(8),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

/// What one attempt says about trying again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The attempt succeeded, or failed in a way that another attempt cannot fix.
    Final,
    /// The attempt failed in a way that may pass on another attempt.
    Transient {
        /// The wait the server asked for, when it asked.
        retry_after: Option<Duration>,
    },
}

/// What the policy decides after an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// Return this attempt's result.
    Stop,
    /// Sleep this long on the clock, then try again.
    RetryAfter(Duration),
}

/// A result of one attempt that can say whether to try again.
pub trait Retryable {
    /// The outcome of the attempt. `now` comes from the policy's clock, for a
    /// `Retry-After` given as a date.
    fn outcome(&self, now: Timestamp) -> Outcome;
}

impl RetryPolicy {
    /// A policy that never retries.
    pub fn none() -> Self {
        RetryPolicy { max_attempts: 1, ..RetryPolicy::default() }
    }

    /// The decision after attempt number `attempt` (1 for the first) ended with
    /// `outcome`. `rng` spreads the backoff so that clients do not retry in lockstep.
    pub fn decide(&self, attempt: u32, outcome: Outcome, rng: &dyn Rng) -> Decision {
        let Outcome::Transient { retry_after } = outcome else {
            return Decision::Stop;
        };
        if attempt >= self.max_attempts {
            return Decision::Stop;
        }
        match retry_after {
            Some(wait) if wait > self.max_retry_after => Decision::Stop,
            Some(wait) => Decision::RetryAfter(wait),
            None => Decision::RetryAfter(self.backoff(attempt, rng)),
        }
    }

    /// The wait after attempt number `attempt` when the server gave none: the
    /// exponential base `initial_backoff * 2^(attempt - 1)`, capped at `max_backoff`,
    /// of which half is fixed and half is drawn from `rng` ("equal jitter").
    pub fn backoff(&self, attempt: u32, rng: &dyn Rng) -> Duration {
        let doublings = attempt.saturating_sub(1).min(31);
        let base = self.initial_backoff.saturating_mul(1 << doublings).min(self.max_backoff);
        let half = base / 2;
        let spread = u64::try_from(half.as_nanos()).unwrap_or(u64::MAX);
        let jitter = match spread.checked_add(1) {
            Some(modulus) => rng.next_u64() % modulus,
            None => rng.next_u64(),
        };
        half.saturating_add(Duration::from_nanos(jitter))
    }

    /// Runs `operation` until it returns a final outcome or the policy stops, and
    /// returns the last result. `operation` receives the attempt number, 1 for the
    /// first. Waits go through `clock`, never through a real timer.
    pub async fn run<T, E, F, Fut>(
        &self,
        clock: &dyn Clock,
        rng: &dyn Rng,
        mut operation: F,
    ) -> Result<T, E>
    where
        T: Retryable,
        E: Retryable,
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut attempt = 1;
        loop {
            let result = operation(attempt).await;
            let outcome = match &result {
                Ok(value) => value.outcome(clock.now()),
                Err(error) => error.outcome(clock.now()),
            };
            match self.decide(attempt, outcome, rng) {
                Decision::Stop => return result,
                Decision::RetryAfter(wait) => {
                    // The response holds a connection; release it before the wait.
                    drop(result);
                    clock.sleep(wait).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// True for the statuses after which the same request may be sent again.
///
/// 408, 429 and 503 say that the server did not handle the request, so they qualify
/// for any request. 500, 502 and 504 come after the server, or a server behind a
/// gateway, may have acted on it, so they qualify only for an `idempotent` request.
pub fn is_retryable_status(status: StatusCode, idempotent: bool) -> bool {
    match status {
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::TOO_MANY_REQUESTS
        | StatusCode::SERVICE_UNAVAILABLE => true,
        StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::GATEWAY_TIMEOUT => idempotent,
        _ => false,
    }
}

/// The wait a response asks for: `retry-after-ms` when present and valid, otherwise
/// `Retry-After` as seconds or as an HTTP date. A date in the past means no wait.
pub fn retry_after(headers: &HeaderMap, now: Timestamp) -> Option<Duration> {
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim);
    if let Some(millis) = text(RETRY_AFTER_MS).and_then(|value| value.parse::<u64>().ok()) {
        return Some(Duration::from_millis(millis));
    }
    let value = text(http::header::RETRY_AFTER.as_str())?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = HTTP_DATE.parse_timestamp(value).ok()?;
    let wait = now.duration_until(at);
    Some(Duration::try_from(wait).unwrap_or(Duration::ZERO))
}

/// One attempt of an HTTP request, with what the policy needs to judge it: whether the
/// request is idempotent. A response or an error alone cannot say whether a second copy
/// of its request is safe, so neither implements [`Retryable`] by itself.
#[derive(Debug)]
pub(crate) struct HttpAttempt<T> {
    pub(crate) value: T,
    pub(crate) idempotent: bool,
}

impl Retryable for HttpAttempt<HttpResponse> {
    fn outcome(&self, now: Timestamp) -> Outcome {
        if is_retryable_status(self.value.status(), self.idempotent) {
            Outcome::Transient { retry_after: retry_after(self.value.headers(), now) }
        } else {
            Outcome::Final
        }
    }
}

impl Retryable for HttpAttempt<HttpError> {
    fn outcome(&self, _now: Timestamp) -> Outcome {
        if self.value.is_retryable(self.idempotent) {
            Outcome::Transient { retry_after: None }
        } else {
            Outcome::Final
        }
    }
}

#[cfg(test)]
mod tests;
