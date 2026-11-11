//! What a provider says about a model call before its answer streams.

use std::time::Duration;

/// One step of a model call before its answer streams, for a person who waits for it:
/// a connect, and each wait before another attempt. Notices are best effort: a provider
/// may send none, and the call's result says how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallNotice {
    /// The provider sends the request, or opens a connection for it, and no answer came
    /// yet.
    Connecting,
    /// The provider's API took the request: the answer follows.
    Connected,
    /// Attempt number `attempt` (1 for the first) failed in a way that another attempt
    /// may fix. The provider waits `wait`, then sends the request again.
    Retrying {
        /// The number of the attempt that failed, from 1.
        attempt: u32,
        /// The wait before the next attempt.
        wait: Duration,
        /// The HTTP status of the failed attempt; `None` when no answer came, such as
        /// when no connection could be made.
        status: Option<u16>,
    },
}

/// Where a provider reports the [`CallNotice`]s of one model call
/// ([`Provider::stream_with_notices`](crate::Provider::stream_with_notices)).
///
/// A provider calls it inside the call, so it must not block: it only hands the notice
/// on.
pub trait CallNotices: Send + Sync {
    /// Takes one notice.
    fn notice(&self, notice: CallNotice);
}

/// Notices that go nowhere: what [`Provider::stream`](crate::Provider::stream) uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNotices;

impl CallNotices for NoNotices {
    fn notice(&self, _notice: CallNotice) {}
}
