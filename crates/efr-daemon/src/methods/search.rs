//! What the two search methods share (`conversations.search` and `conversation.search`):
//! the time caps of a search, its stop, the error that a client sees, and the titles
//! that a row shows.
//!
//! A search runs on a search lane of the store (`efr_store::search::SearchLanes`),
//! never on one of the readers of the turns, so a burst of keys cannot slow a prompt.
//! Its time counts from the start of the handler, the wait for a lane included, on the
//! injected clock: at [`SOFT_CAP`] the row loop of the store ends at the next row and
//! the search ranks what it has seen; at [`HARD_CAP`] efrd answers with no rows, says
//! `time_cap`, and interrupts the statement of the lane. One step inside FTS5 reads no
//! interrupt, so the lane can stay busy until that step ends; the answer does not wait
//! for it. When the client cancels or goes away, the transport drops the handler, and
//! [`Run`]'s drop stops the search the same way.
//!
//! An error of the store on this path never carries the query or a doc: the client
//! and the log see one fixed sentence ([`DaemonError::SearchFailed`]).

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use efr_protocol::{PageCursor, RecentConversation, SearchHit, SearchPartial};
use efr_stdx::time::{Clock, Sleep, Stopwatch};
use efr_store::StoreError;
use efr_store::search::{DocKind, FoundHit, SearchStop};

use crate::DaemonError;
use crate::methods::cursor;
use crate::search_index::index_text;
use crate::state::State;

/// When the row loop of a search ends and the search ranks what it has seen.
pub(crate) const SOFT_CAP: Duration = Duration::from_millis(150);

/// When efrd answers whatever the search has done: no rows when it has not ended.
/// It is also the longest wait for a lane, after which a search is `busy`.
pub(crate) const HARD_CAP: Duration = Duration::from_millis(200);

/// What a title ends with when efr cut it (`efr_store`'s `title_from`).
const CUT: &str = "...";

/// A hook that runs after every `n` rows of the row loop of each search, so a test can
/// hold a search between two rows while it moves the clock
/// (`efr_store::search::SearchStop::on_rows`). Production has none.
#[derive(Clone)]
pub struct SearchRowHook {
    every: u32,
    hook: Arc<dyn Fn(u32) + Send + Sync>,
}

impl SearchRowHook {
    /// Runs `hook` with the count of rows after every `every` rows.
    pub fn new(every: u32, hook: Arc<dyn Fn(u32) + Send + Sync>) -> Self {
        SearchRowHook { every, hook }
    }
}

impl fmt::Debug for SearchRowHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchRowHook").field("every", &self.every).finish_non_exhaustive()
    }
}

/// How a search on a lane ended.
#[derive(Debug)]
pub(crate) enum Ran<T> {
    /// The work ended in time, with what it found.
    Done(T),
    /// [`HARD_CAP`] came first: the answer has no rows.
    TimeCap,
}

/// One search: its stop, its two caps on the clock, and its stopwatch. Dropping it
/// stops the search hard, so a handler that the transport drops frees its lane.
pub(crate) struct Run {
    stop: SearchStop,
    started: Arc<AtomicBool>,
    soft: Sleep,
    hard: Sleep,
    watch: Stopwatch,
}

impl Run {
    /// Starts the caps of a search now.
    pub(crate) fn start(state: &State) -> Run {
        Run::on(&*state.clock, state.search_hook.as_ref())
    }

    /// Starts the caps of a search now on `clock`, with a test's `hook`.
    fn on(clock: &dyn Clock, hook: Option<&SearchRowHook>) -> Run {
        let stop = SearchStop::new();
        let stop = match hook {
            Some(SearchRowHook { every, hook }) => stop.on_rows(*every, Arc::clone(hook)),
            None => stop,
        };
        Run {
            stop,
            started: Arc::new(AtomicBool::new(false)),
            soft: clock.sleep(SOFT_CAP),
            hard: clock.sleep(HARD_CAP),
            watch: Stopwatch::start(),
        }
    }

    /// The stop that the work on the lane checks.
    pub(crate) fn stop(&self) -> SearchStop {
        self.stop.clone()
    }

    /// The mark that the work sets when it starts on its lane: a search that has no
    /// lane at [`HARD_CAP`] is `busy`, not `time_cap`.
    pub(crate) fn started(&self) -> Started {
        Started(Arc::clone(&self.started))
    }

    /// The time since the start, for the measures of a search.
    pub(crate) fn elapsed(&self) -> Duration {
        self.watch.elapsed()
    }

    /// Waits for `work` (a `SearchLanes::with` of this search) under the caps.
    pub(crate) async fn finish<T>(
        &mut self,
        work: impl Future<Output = Result<T, StoreError>>,
    ) -> Result<Ran<T>, DaemonError> {
        let mut work = std::pin::pin!(work);
        tokio::select! {
            biased;
            result = &mut work => return result.map(Ran::Done).map_err(search_error),
            () = &mut self.soft => self.stop.soft(),
        }
        tokio::select! {
            biased;
            result = &mut work => result.map(Ran::Done).map_err(search_error),
            () = &mut self.hard => {
                self.stop.hard();
                if self.started.load(Ordering::Acquire) {
                    Ok(Ran::TimeCap)
                } else {
                    Err(DaemonError::SearchBusy)
                }
            }
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // NOTE: also after an answer: the work has then ended, or it runs on after the
        // hard cap, and this ends it at its next row or opcode.
        self.stop.hard();
    }
}

impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run").field("stop", &self.stop).finish_non_exhaustive()
    }
}

/// The mark of a search that got its lane.
#[derive(Debug, Clone)]
pub(crate) struct Started(Arc<AtomicBool>);

impl Started {
    /// Marks the search as started on its lane.
    pub(crate) fn now(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The error that a client sees for a failed search. The query limits, a busy lane
/// and a missing conversation keep their own codes; every other error of the store is
/// one fixed sentence, so neither the answer nor the log shows the query.
pub(crate) fn search_error(error: StoreError) -> DaemonError {
    match error {
        StoreError::QueryTooLong { .. }
        | StoreError::QueryTooManyTerms { .. }
        | StoreError::SearchBusy
        | StoreError::SearchStopped
        | StoreError::UnknownConversation { .. } => DaemonError::from(error),
        source => DaemonError::SearchFailed { source },
    }
}

/// The wire hit of a hit of the store, with a history cursor whose page ends at it.
pub(crate) fn wire_hit(hit: FoundHit) -> SearchHit {
    SearchHit {
        kind: hit.kind,
        seq: hit.seq,
        turn_id: hit.turn_id,
        at: hit.at,
        history_cursor: history_cursor(hit.seq),
        lines: hit.lines,
    }
}

/// A `conversation.history` cursor whose page ends at `seq`: the page holds the
/// events before the cursor's sequence number.
fn history_cursor(seq: efr_protocol::Seq) -> PageCursor {
    cursor(efr_protocol::Seq::new(seq.get().saturating_add(1)))
}

/// The title that a row of a search shows: the search title, from the indexed text of
/// the first prompt, else the projection's title with the secrets that the index
/// redaction recognises replaced. A projection title that efr cut at 80 characters
/// loses its last word too, so a key that the cut made too short to be recognised
/// never shows.
pub(crate) fn shown_title(search: Option<String>, projection: Option<&str>) -> Option<String> {
    search.or_else(|| projection.and_then(redacted_title))
}

/// `conversation` with the title that a row of `,resume` shows: in the picker, in
/// `efr search` and in the list without the picker. It is [`shown_title`] of
/// `search_title` and the projection's title.
pub(crate) fn titled(
    mut conversation: RecentConversation,
    search_title: Option<String>,
) -> RecentConversation {
    conversation.summary.title = shown_title(search_title, conversation.summary.title.as_deref());
    conversation
}

fn redacted_title(title: &str) -> Option<String> {
    let Some(kept) = title.strip_suffix(CUT) else {
        return index_text(DocKind::Prompt, title);
    };
    let kept = kept.trim_end();
    let kept = kept.rfind(char::is_whitespace).map_or("", |at| &kept[..at]);
    Some(match index_text(DocKind::Prompt, kept) {
        Some(text) => format!("{} {CUT}", text.trim_end()),
        None => CUT.to_owned(),
    })
}

/// The reason in a debug line of a search, never the query.
pub(crate) fn partial_name(partial: Option<SearchPartial>) -> &'static str {
    partial.map_or("none", SearchPartial::as_str)
}

#[cfg(test)]
mod tests;
