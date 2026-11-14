//! The limits that each account reached, for the switch offer of a turn whose account
//! is out (`turn_failed`), for `models.list` and for `admin.status`.
//!
//! A model call that fails with `ProviderError::LimitReached` notes the limit of its
//! account here, through [`Limited`], the wrapper of every provider that efrd runs, the
//! calls of the reviewer included. The first event of a later call of the same account
//! clears the note, because the account serves again. A limit with a reset ends at its
//! reset time; one without a reset, such as no credit, holds until a call works again.
//! A login or a logout of the account clears it. "Too many requests" is no limit of the
//! account (`LimitKind::TooManyRequests` is only in `account_out`), so it is never
//! noted. efrd keeps the notes in memory only: after a restart it knows of no limit
//! until a call meets one again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_protocol::{AccountLimits, LimitKind, LimitReached};
use efr_provider::{
    CallNotices, EditTool, ModelInfo, NoNotices, Provider, ProviderError, ProviderId,
    ProviderStream, Request, Warmup, WebSupport,
};
use efr_stdx::time::Clock;
use futures::StreamExt as _;
use jiff::Timestamp;

/// The limit that each account reached, by provider id.
#[derive(Debug)]
pub(crate) struct LimitBook {
    clock: Arc<dyn Clock>,
    notes: Mutex<HashMap<String, Note>>,
}

/// One limit that an account reached, and when efr learned it.
#[derive(Debug, Clone)]
struct Note {
    observed_at: Timestamp,
    reached: LimitReached,
}

impl LimitBook {
    /// No limit yet; the times come from `clock`.
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Self {
        LimitBook { clock, notes: Mutex::default() }
    }

    /// Notes that `provider` reached `reached` now. A note of "too many requests"
    /// changes nothing.
    pub(crate) fn note(&self, provider: &str, reached: &LimitReached) {
        if reached.kind == LimitKind::TooManyRequests {
            return;
        }
        let note = Note { observed_at: self.clock.now(), reached: reached.clone() };
        let first = self.lock().insert(provider.to_owned(), note).is_none();
        if first {
            tracing::info!(provider = %provider, limit = ?reached.kind, resets_at = ?reached.resets_at, "the account reached a limit");
        }
    }

    /// Forgets the limit of `provider`: a call of it works again, or a login or a
    /// logout changed its credential.
    pub(crate) fn clear(&self, provider: &str) {
        self.lock().remove(provider);
    }

    /// The limit that still stops `provider` now; a limit whose reset time has passed
    /// is gone.
    pub(crate) fn reached(&self, provider: &str) -> Option<LimitReached> {
        self.note_of(provider).map(|note| note.reached)
    }

    /// The limits of `provider` for a client: the limit that still stops it, with the
    /// time efr learned it. `None` while efr knows of none.
    pub(crate) fn limits(&self, provider: &str) -> Option<AccountLimits> {
        self.note_of(provider).map(|note| AccountLimits {
            observed_at: note.observed_at,
            windows: Vec::new(),
            reached: Some(note.reached),
        })
    }

    fn note_of(&self, provider: &str) -> Option<Note> {
        let now = self.clock.now();
        let mut notes = self.lock();
        let note = notes.get(provider)?.clone();
        if note.reached.resets_at.is_some_and(|at| at <= now) {
            notes.remove(provider);
            return None;
        }
        Some(note)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Note>> {
        // NOTE: every change replaces whole values, so a poisoned map is still usable.
        self.notes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A provider whose model calls note in [`LimitBook`] when the account reaches a limit,
/// and clear it when a call works again. The answer itself is passed on as it comes.
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
        if let ProviderError::LimitReached { reached, .. } = error {
            self.book.note(&self.provider, reached);
        }
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
        Ok(Box::pin(stream.inspect(move |item| match item {
            Ok(_) if first => {
                first = false;
                book.clear(&provider);
            }
            Ok(_) => {}
            Err(ProviderError::LimitReached { reached, .. }) => book.note(&provider, reached),
            Err(_) => {}
        })))
    }

    async fn warm(&self, warmup: &Warmup) -> bool {
        self.inner.warm(warmup).await
    }
}

#[cfg(test)]
mod tests;
