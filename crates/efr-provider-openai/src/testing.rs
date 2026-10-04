//! Fakes shared by the unit tests of this crate: a clock, a random source, a token
//! source and the fixture loader.
//!
//! The clock stays here instead of coming from `efr-test-support`: its `TestClock` can
//! stand in only for a clock that nobody moves, not for one whose sleeps end at once,
//! and `efr-test-support` brings `efr-store` with its bundled SQLite build, which about
//! triples the time to build these tests from clean.

use std::future::ready;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use efr_http::{SseDecoder, SseEvent};
use efr_provider::{AccessToken, ProviderError, SecretString, TokenSource};
use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Sleep};
use jiff::{SignedDuration, Timestamp};

/// The instant the fake clock starts at: 2026-10-04T12:00:00Z.
pub(crate) fn start() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

/// A clock whose sleeps finish at once and move `now` forward by their duration.
#[derive(Debug)]
pub(crate) struct InstantClock {
    now: Mutex<Timestamp>,
    sleeps: Mutex<Vec<Duration>>,
}

impl InstantClock {
    pub(crate) fn new() -> Self {
        InstantClock { now: Mutex::new(start()), sleeps: Mutex::new(Vec::new()) }
    }

    /// Every sleep so far, in order.
    pub(crate) fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        self.sleeps.lock().unwrap().push(duration);
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(SignedDuration::try_from(duration).unwrap()).unwrap();
        Box::pin(ready(()))
    }
}

/// A generator that always draws the same number.
#[derive(Debug)]
pub(crate) struct FixedRng(pub(crate) u64);

impl Rng for FixedRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for (index, byte) in dest.iter_mut().enumerate() {
            *byte = self.0.to_le_bytes()[index % 8];
        }
    }

    fn next_u64(&self) -> u64 {
        self.0
    }
}

/// A token source that hands out its tokens in order, moving to the next one on each
/// `invalidate`, and counts the calls.
#[derive(Debug)]
pub(crate) struct FakeTokens {
    tokens: Vec<&'static str>,
    account_id: Option<&'static str>,
    fetches: AtomicUsize,
    invalidations: AtomicUsize,
}

impl FakeTokens {
    /// A source of `tokens`; the last one repeats once the others are invalidated.
    pub(crate) fn new(tokens: &[&'static str]) -> Self {
        FakeTokens {
            tokens: tokens.to_vec(),
            account_id: None,
            fetches: AtomicUsize::new(0),
            invalidations: AtomicUsize::new(0),
        }
    }

    pub(crate) fn with_account_id(mut self, account_id: &'static str) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub(crate) fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    pub(crate) fn invalidations(&self) -> usize {
        self.invalidations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl TokenSource for FakeTokens {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let index = self.invalidations().min(self.tokens.len().saturating_sub(1));
        let Some(token) = self.tokens.get(index) else {
            return Err(ProviderError::NotLoggedIn);
        };
        let token = AccessToken::new(SecretString::from(*token));
        Ok(match self.account_id {
            Some(account_id) => token.with_account_id(account_id),
            None => token,
        })
    }

    async fn invalidate(&self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }
}

/// The path of the fixture `name` under `fixtures/responses/`.
pub(crate) fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join("responses").join(name)
}

/// The text of the fixture `name` under `fixtures/responses/`.
pub(crate) fn fixture(name: &str) -> String {
    let path = fixture_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The server-sent events of the fixture `name`.
pub(crate) fn fixture_events(name: &str) -> Vec<SseEvent> {
    let mut decoder = SseDecoder::new();
    decoder.push(fixture(name).as_bytes()).unwrap()
}
