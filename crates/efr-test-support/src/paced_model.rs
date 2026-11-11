//! [`PacedModel`]: a model for the streaming benches that hands over each delta of its
//! answer at a set real time after the call.
//!
//! A task of its own sends the deltas into a queue without a limit, as a network
//! socket takes bytes while its reader is busy. So a stream loop that does not read for
//! a time finds the deltas of that time waiting, and the schedule does not move. The
//! model notes when it handed over each delta ([`PacedCall`]), in the time of the mark
//! file (`efr_stdx::mark::now_ns`), so a bench can line its notes up with the marks of
//! efrd and `efr`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_stdx::mark;
use efr_stdx::time::{Clock as _, SystemClock};
use tokio::sync::mpsc;

/// The provider id of a [`PacedModel`], the one that a custom provider of a test
/// daemon has.
const ID: &str = "test";

/// One delta of a paced answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacedDelta {
    /// The real time after the call at which the model hands the delta over.
    pub at: Duration,
    /// True for a delta of the reasoning, false for one of the text.
    pub reasoning: bool,
    /// The delta.
    pub text: String,
}

/// What one model call did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacedCall {
    /// When the call came, in the time of `efr_stdx::mark::now_ns`.
    pub called_ns: u64,
    /// For each delta that went out: its marker plus 1 (0 without one, see
    /// `efr_stdx::mark::scan`), true for reasoning, and when it went out.
    pub sent: Vec<(u64, bool, u64)>,
    /// When the end of the answer went out; 0 before that.
    pub done_ns: u64,
}

/// A model that answers each call with the next script of its queue, on the script's
/// schedule. A call that finds no script gets the text `ok` at once.
#[derive(Debug)]
pub struct PacedModel {
    id: ProviderId,
    scripts: Mutex<VecDeque<Vec<PacedDelta>>>,
    calls: Arc<Mutex<Vec<PacedCall>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PacedModel {
    /// A model with no script.
    pub fn new() -> Arc<PacedModel> {
        let id = match ProviderId::new(ID) {
            Ok(id) => id,
            Err(_) => unreachable!("`test` is lowercase ASCII letters, a valid provider id"),
        };
        Arc::new(PacedModel { id, scripts: Mutex::default(), calls: Arc::default() })
    }

    /// Queues `script` for the next call that finds no script before it.
    pub fn push(&self, script: Vec<PacedDelta>) {
        lock(&self.scripts).push_back(script);
    }

    /// The calls so far, in order.
    pub fn calls(&self) -> Vec<PacedCall> {
        lock(&self.calls).clone()
    }
}

#[async_trait]
impl Provider for PacedModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let script = lock(&self.scripts).pop_front().unwrap_or_else(|| {
            vec![PacedDelta { at: Duration::ZERO, reasoning: false, text: "ok".to_owned() }]
        });
        let index = {
            let mut calls = lock(&self.calls);
            calls.push(PacedCall { called_ns: mark::now_ns(), ..PacedCall::default() });
            calls.len() - 1
        };
        let calls = Arc::clone(&self.calls);
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let start = Instant::now();
            for delta in script {
                let wait = delta.at.saturating_sub(start.elapsed());
                if !wait.is_zero() {
                    SystemClock.sleep(wait).await;
                }
                let marker = mark::scan(delta.text.as_bytes());
                let event = if delta.reasoning {
                    ProviderEvent::ReasoningDelta { text: delta.text }
                } else {
                    ProviderEvent::TextDelta { text: delta.text }
                };
                if let Some(call) = lock(&calls).get_mut(index) {
                    call.sent.push((marker, delta.reasoning, mark::now_ns()));
                }
                if tx.send(Ok(event)).is_err() {
                    return;
                }
            }
            if let Some(call) = lock(&calls).get_mut(index) {
                call.done_ns = mark::now_ns();
            }
            let done = ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None };
            let _ = tx.send(Ok(done));
        });
        let events = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        });
        Ok(Box::pin(events))
    }
}
