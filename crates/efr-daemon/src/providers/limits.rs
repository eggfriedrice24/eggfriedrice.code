//! The limits of each account: how full the usage windows of its plan are, and the
//! limit that stopped it, for `models.list`, for `admin.status` and for the switch offer
//! of a turn whose account is out (`turn_failed`).
//!
//! [`Limited`], the wrapper of every provider that efrd runs, the calls of the reviewer
//! included, notes here what the answers of the account tell:
//!
//! - The usage windows of a ChatGPT plan come with each answer
//!   (`ProviderEvent::Limits`). Each report replaces the windows of the account. A
//!   window whose reset time has passed is gone.
//! - A call that fails with `ProviderError::LimitReached`, or with a rate limit whose
//!   headers named the bucket that ran out (`ProviderError::RateLimited` with a
//!   `limit`), notes the limit of the account. The first event of a later call of the
//!   same account clears it, because the account serves again. A limit with a reset
//!   ends at its reset time; one without a reset, such as no credit, holds until a call
//!   works again. "Too many requests" with no bucket is no limit of the account
//!   (`LimitKind::TooManyRequests` is only in `account_out`), so it is never noted.
//!   The rate limit of an API key thus shows only when it was hit.
//!
//! A login or a logout of the account clears all of it. efrd keeps the notes in memory
//! only: after a restart it knows of no limit until a call tells one again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_protocol::{AccountLimits, LimitKind, LimitReached, LimitWindow};
use efr_provider::{
    CallNotices, EditTool, ModelInfo, NoNotices, Provider, ProviderError, ProviderEvent,
    ProviderId, ProviderStream, Request, Warmup, WebSupport,
};
use efr_stdx::time::Clock;
use futures::StreamExt as _;
use jiff::Timestamp;

/// The limits of each account, by provider id.
#[derive(Debug)]
pub(crate) struct LimitBook {
    clock: Arc<dyn Clock>,
    accounts: Mutex<HashMap<String, Account>>,
}

/// What efr knows of the limits of one account.
#[derive(Debug, Clone, Default)]
struct Account {
    /// The usage windows of the newest answer that told them, and when it came.
    windows: Option<(Timestamp, Vec<LimitWindow>)>,
    /// The limit that stopped a call of the account, and when the call failed.
    reached: Option<(Timestamp, LimitReached)>,
}

impl Account {
    /// The account without what ended before `now`: each window and the limit whose
    /// reset time has passed.
    fn at(&mut self, now: Timestamp) {
        let passed = |resets_at: Option<Timestamp>| resets_at.is_some_and(|at| at <= now);
        if let Some((_, windows)) = &mut self.windows {
            windows.retain(|window| !passed(window.resets_at));
            if windows.is_empty() {
                self.windows = None;
            }
        }
        if self.reached.as_ref().is_some_and(|(_, reached)| passed(reached.resets_at)) {
            self.reached = None;
        }
    }

    fn is_empty(&self) -> bool {
        self.windows.is_none() && self.reached.is_none()
    }
}

impl LimitBook {
    /// No limit yet; the times come from `clock`.
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Self {
        LimitBook { clock, accounts: Mutex::default() }
    }

    /// Notes that `provider` reached `reached` now. A note of "too many requests"
    /// changes nothing.
    pub(crate) fn note(&self, provider: &str, reached: &LimitReached) {
        if reached.kind == LimitKind::TooManyRequests {
            return;
        }
        let now = self.clock.now();
        let first = self
            .lock()
            .entry(provider.to_owned())
            .or_default()
            .reached
            .replace((now, reached.clone()))
            .is_none();
        if first {
            tracing::info!(provider = %provider, limit = ?reached.kind, resets_at = ?reached.resets_at, "the account reached a limit");
        }
    }

    /// Notes the usage `windows` that an answer of `provider` told now. They replace
    /// the windows of the account; an empty report changes nothing.
    pub(crate) fn note_windows(&self, provider: &str, windows: &[LimitWindow]) {
        if windows.is_empty() {
            return;
        }
        let now = self.clock.now();
        self.lock().entry(provider.to_owned()).or_default().windows = Some((now, windows.to_vec()));
    }

    /// A call of `provider` works: the limit that stopped it is gone. Its windows stay
    /// until a newer answer tells others.
    pub(crate) fn served(&self, provider: &str) {
        let mut accounts = self.lock();
        if let Some(account) = accounts.get_mut(provider) {
            account.reached = None;
            if account.is_empty() {
                accounts.remove(provider);
            }
        }
    }

    /// Forgets all that efr knows of the limits of `provider`: a login or a logout
    /// changed its credential.
    pub(crate) fn clear(&self, provider: &str) {
        self.lock().remove(provider);
    }

    /// The limit that stops `provider` now: the one that stopped its last call, else,
    /// when a window of its plan is full, the usage limit of the plan, until the last
    /// full window resets. A limit whose reset time has passed is gone.
    pub(crate) fn reached(&self, provider: &str) -> Option<LimitReached> {
        let account = self.account(provider)?;
        if let Some((_, reached)) = account.reached {
            return Some(reached);
        }
        let (_, windows) = account.windows?;
        let full: Vec<_> = windows.iter().filter(|window| window.used_percent >= 100).collect();
        if full.is_empty() {
            return None;
        }
        // NOTE: a full window without a reset time ends at no known time.
        let resets_at = full.iter().map(|window| window.resets_at).collect::<Option<Vec<_>>>();
        let resets_at = resets_at.and_then(|times| times.into_iter().max());
        Some(LimitReached { kind: LimitKind::Usage, resets_at })
    }

    /// The limits of `provider` for a client: its usage windows and the limit that
    /// stopped it, with the time efr learned them: that of the windows while there are
    /// any, else that of the limit. `None` while efr knows of none.
    pub(crate) fn limits(&self, provider: &str) -> Option<AccountLimits> {
        let Account { windows, reached } = self.account(provider)?;
        let observed_at = match (&windows, &reached) {
            (Some((at, _)), _) | (None, Some((at, _))) => *at,
            (None, None) => return None,
        };
        Some(AccountLimits {
            observed_at,
            windows: windows.map(|(_, windows)| windows).unwrap_or_default(),
            reached: reached.map(|(_, reached)| reached),
        })
    }

    /// What efr knows of `provider` now, without what ended; `None` when that is
    /// nothing.
    fn account(&self, provider: &str) -> Option<Account> {
        let now = self.clock.now();
        let mut accounts = self.lock();
        let account = accounts.get_mut(provider)?;
        account.at(now);
        if account.is_empty() {
            accounts.remove(provider);
            return None;
        }
        Some(account.clone())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Account>> {
        // NOTE: every change replaces whole values, so a poisoned map is still usable.
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A provider whose model calls note in [`LimitBook`] what the answers tell of the
/// limits of the account: its usage windows, the limit that stopped a call, and that
/// a call works again. The answer itself is passed on as it comes, the windows
/// included, so a turn can show them.
#[derive(Debug)]
pub(crate) struct Limited {
    inner: Arc<dyn Provider>,
    /// The provider id of the account, such as `openai-subscription.b`.
    provider: String,
    book: Arc<LimitBook>,
}

impl Limited {
    /// `inner`, the account `provider`, noting its limits in `book`.
    pub(crate) fn new(inner: Arc<dyn Provider>, provider: &str, book: Arc<LimitBook>) -> Self {
        Limited { inner, provider: provider.to_owned(), book }
    }

    fn failed(&self, error: &ProviderError) {
        note_error(&self.book, &self.provider, error);
    }
}

/// Notes the limit of the account `provider` that `error` names, if it names one.
fn note_error(book: &LimitBook, provider: &str, error: &ProviderError) {
    match error {
        ProviderError::LimitReached { reached, .. }
        | ProviderError::RateLimited { limit: Some(reached), .. } => book.note(provider, reached),
        _ => {}
    }
}

#[async_trait]
impl Provider for Limited {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.inner.models()
    }

    fn default_edit_tool(&self) -> EditTool {
        self.inner.default_edit_tool()
    }

    // NOTE: the default of the trait is no web tool, so a wrapper that does not pass it
    // on would keep web search out of every request of its provider.
    fn web_search(&self, effort: Option<&str>) -> WebSupport {
        self.inner.web_search(effort)
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.stream_with_notices(request, &NoNotices).await
    }

    async fn stream_with_notices(
        &self,
        request: Request,
        notices: &dyn CallNotices,
    ) -> Result<ProviderStream, ProviderError> {
        let stream = self
            .inner
            .stream_with_notices(request, notices)
            .await
            .inspect_err(|error| self.failed(error))?;
        let book = Arc::clone(&self.book);
        let provider = self.provider.clone();
        let mut first = true;
        Ok(Box::pin(stream.inspect(move |item| {
            if item.is_ok() && first {
                first = false;
                book.served(&provider);
            }
            match item {
                Ok(ProviderEvent::Limits { windows }) => book.note_windows(&provider, windows),
                Err(error) => note_error(&book, &provider, error),
                Ok(_) => {}
            }
        })))
    }

    async fn warm(&self, warmup: &Warmup) -> bool {
        self.inner.warm(warmup).await
    }
}

#[cfg(test)]
mod tests;
