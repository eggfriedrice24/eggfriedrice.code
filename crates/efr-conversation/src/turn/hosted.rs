//! The steps of the provider's web tool in one turn: their events, the count of the
//! searches, the numbered sources and the marks in the text.
//!
//! The provider runs a hosted step inside the model call, so efr never authorizes it and
//! never runs it. The turn only records what the stream says: `web_call_started` and
//! `web_call_completed` for each step, and at the end of the turn one
//! `turn_web_report` with the count and the cited pages (the build spec of web search,
//! 3.3, 3.4 and 3.11).
//!
//! - **The calls.** Each step gets efr's own [`CallId`] at its start. The open steps stay
//!   here across the model calls of the turn, in the order of their starts, so the end
//!   of a step that a pause or a client tool call moved to the next answer finds its
//!   start. An end whose start never came opens and closes the step in one batch.
//! - **The count.** A search that ended without an error adds one. Failed searches,
//!   opens and finds add nothing (DECISIONS-web 3), so the count always equals the
//!   `✓ web` rows of a search that the user saw.
//! - **The sources.** A cited URL that is new to the turn gets the next number; a known
//!   URL keeps its number. The URLs compare as strings. At most
//!   [`MAX_WEB_SOURCES`] get a number.
//! - **The marks.** A placed citation (Claude) puts ` [[n]](<url>)` into the text, right
//!   after the words that it backs. Two citations of one run that have the same number
//!   give one mark. An unplaced citation (OpenAI today) gets only its number.
//!
//! The module does no IO: each method returns the events that the turn records.

use std::sync::atomic::{AtomicBool, Ordering};

use efr_protocol::{
    CallId, Event, MAX_WEB_RESULTS, MAX_WEB_SOURCES, MAX_WEB_TEXT_CHARS, MAX_WEB_TITLE_CHARS,
    MAX_WEB_URL_CHARS, TurnId, WebAction, WebError, WebResult, WebSource,
};
use efr_provider::{HostedAction, HostedError, HostedOutcome};

use crate::web::WebPlan;

/// The most continuations of a turn that the provider paused (`pause_turn`). After them
/// the turn ends as a turn that stopped for the pause.
pub(crate) const MAX_PAUSES: u32 = 5;

/// One step that started and did not end yet.
#[derive(Debug, Clone)]
struct OpenCall {
    /// The provider's id, which the end of the step names.
    provider_id: String,
    call_id: CallId,
    action: HostedAction,
    /// True when the step started in the newest answer.
    this_answer: bool,
}

/// What the turn did with the provider's web tool so far.
#[derive(Debug)]
pub(super) struct WebState {
    /// The plan of the turn: which web the requests carry, or why none.
    pub(super) plan: WebPlan,
    /// The steps that started and did not end yet, in the order of their starts.
    open: Vec<OpenCall>,
    /// The searches that ended without an error.
    pub(super) searches: u32,
    /// The cited pages, in the order of the first citation: source `n` is entry `n - 1`.
    sources: Vec<WebSource>,
    /// The URL of each source as the provider gave it, for the compare: the source
    /// itself holds the cut URL.
    cited: Vec<String>,
    /// The numbers that marks placed since the model's last text.
    marked: Vec<usize>,
    /// The continuations of a paused turn so far.
    pub(super) pauses: u32,
    /// True when the next request must end as the answer before it did: no note and no
    /// steer may come between a paused answer, or a hosted step that waits, and the
    /// call that goes on with it.
    pub(super) hold: bool,
    /// True when an earlier turn of the conversation said what the stored copy means.
    noted_before: bool,
}

impl WebState {
    pub(super) fn new(plan: WebPlan) -> Self {
        WebState {
            plan,
            open: Vec::new(),
            searches: 0,
            sources: Vec::new(),
            cited: Vec::new(),
            marked: Vec::new(),
            pauses: 0,
            hold: false,
            noted_before: false,
        }
    }

    /// An earlier turn of the conversation already had the note of the stored copy.
    pub(super) fn noted_before(&mut self) {
        self.noted_before = true;
    }

    /// A new answer starts: the steps that are open now started in an earlier one.
    pub(super) fn begin_answer(&mut self) {
        for call in &mut self.open {
            call.this_answer = false;
        }
        self.marked.clear();
    }

    /// True when a step of the newest answer started and did not end: the provider runs
    /// it in the next call, which must follow the answer with nothing between.
    pub(super) fn pending(&self) -> bool {
        self.open.iter().any(|call| call.this_answer)
    }

    /// The model wrote more text: a later citation backs other words.
    pub(super) fn text_grew(&mut self) {
        self.marked.clear();
    }

    /// The step `provider_id` started with `action`, as `call_id`: its
    /// `web_call_started`. `None` for a second start of a step that is open.
    pub(super) fn start(
        &mut self,
        turn_id: TurnId,
        provider_id: &str,
        action: &HostedAction,
        call_id: CallId,
    ) -> Option<Event> {
        if self.open.iter().any(|call| call.provider_id == provider_id) {
            return None;
        }
        self.open.push(OpenCall {
            provider_id: provider_id.to_owned(),
            call_id,
            action: action.clone(),
            this_answer: true,
        });
        Some(started(turn_id, call_id, action))
    }

    /// The step `provider_id` ended: its `web_call_completed`, after its
    /// `web_call_started` as `mint()` when its start never came. A search that ended
    /// without an error counts.
    pub(super) fn end(
        &mut self,
        turn_id: TurnId,
        provider_id: &str,
        action: &HostedAction,
        outcome: &HostedOutcome,
        mint: impl FnOnce() -> CallId,
    ) -> Vec<Event> {
        let mut events = Vec::with_capacity(2);
        let (call_id, action) =
            match self.open.iter().position(|call| call.provider_id == provider_id) {
                Some(at) => {
                    let call = self.open.remove(at);
                    (call.call_id, completed_action(&call.action, action))
                }
                None => {
                    let call_id = mint();
                    events.push(started(turn_id, call_id, action));
                    (call_id, action.clone())
                }
            };
        let (results, error) = outcome_parts(outcome);
        if matches!(action, HostedAction::Search { .. }) && error.is_none() {
            self.searches = self.searches.saturating_add(1);
        }
        events.push(completed(turn_id, call_id, &action, results, error));
        events
    }

    /// The answer cites `url`: gives it its number, and returns the mark to put into
    /// the text when the citation is `placed` and the run has no mark of that number
    /// yet. `None` for an unplaced citation, and for a page beyond
    /// [`MAX_WEB_SOURCES`].
    pub(super) fn cite(&mut self, url: &str, title: Option<&str>, placed: bool) -> Option<String> {
        let number = match self.cited.iter().position(|seen| seen == url) {
            Some(at) => at + 1,
            None if self.sources.len() < MAX_WEB_SOURCES => {
                self.cited.push(url.to_owned());
                self.sources.push(WebSource {
                    url: cut_chars(url, MAX_WEB_URL_CHARS),
                    title: title.map(|title| cut_chars(title, MAX_WEB_TITLE_CHARS)),
                });
                self.sources.len()
            }
            None => return None,
        };
        if !placed || self.marked.contains(&number) {
            return None;
        }
        self.marked.push(number);
        let url = self.sources.get(number - 1).map_or(url, |source| source.url.as_str());
        Some(mark(number, url))
    }

    /// The `web_call_completed` of each step that is still open, with `not_finished`:
    /// the turn ended first.
    pub(super) fn unfinished(&mut self, turn_id: TurnId) -> Vec<Event> {
        let outcome = HostedOutcome::Failed { error: HostedError::NotFinished };
        self.open
            .drain(..)
            .map(|call| {
                let (results, error) = outcome_parts(&outcome);
                completed(turn_id, call.call_id, &call.action, results, error)
            })
            .collect()
    }

    /// The `turn_web_report` of the turn, when it ran a search or cited a page. With a
    /// stored copy, the note is on when no earlier turn of the conversation had it and
    /// `noted` (the conversation's flag, which the first such report sets) was not set.
    pub(super) fn report(&self, turn_id: TurnId, noted: &AtomicBool) -> Option<Event> {
        if self.searches == 0 && self.sources.is_empty() {
            return None;
        }
        let stored_copy = self.plan == WebPlan::Stored && self.searches > 0;
        let stored_copy_note =
            stored_copy && !self.noted_before && !noted.swap(true, Ordering::AcqRel);
        Some(Event::TurnWebReport {
            turn_id,
            searches: self.searches,
            stored_copy,
            stored_copy_note,
            sources: self.sources.clone(),
        })
    }
}

/// True when `event` is a report of searches in the stored copy: a client showed the
/// note of the stored copy for the first such report of a conversation.
pub(super) fn notes_stored_copy(event: &Event) -> bool {
    matches!(event, Event::TurnWebReport { stored_copy: true, .. })
}

/// The mark of source `number` at `url`: ` [[n]](<url>)`, a Markdown link whose text is
/// `[n]`. The destination is in angle brackets, with `<`, `>`, `\`, white space and
/// control characters percent-encoded, so any URL is a valid destination: Markdown reads
/// `\>` as an escaped `>`, which would leave the link open.
pub(super) fn mark(number: usize, url: &str) -> String {
    let mut out = format!(" [[{number}]](<");
    for c in url.chars() {
        if matches!(c, '<' | '>' | '\\') || c.is_whitespace() || c.is_control() {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out.push_str(">)");
    out
}

/// The action of an ended step: the end's, with the subject of the start where the end
/// lacks it.
fn completed_action(start: &HostedAction, end: &HostedAction) -> HostedAction {
    match (start, end) {
        (HostedAction::Search { query: Some(query) }, HostedAction::Search { query: None }) => {
            HostedAction::Search { query: Some(query.clone()) }
        }
        (HostedAction::Open { url: Some(url) }, HostedAction::Open { url: None }) => {
            HostedAction::Open { url: Some(url.clone()) }
        }
        (HostedAction::Find { url: Some(url), .. }, HostedAction::Find { pattern, url: None }) => {
            HostedAction::Find { pattern: pattern.clone(), url: Some(url.clone()) }
        }
        _ => end.clone(),
    }
}

/// The kind and the subject of `action`, cut for an event: the action, the query, the
/// page and the pattern.
fn action_parts(
    action: &HostedAction,
) -> (WebAction, Option<String>, Option<String>, Option<String>) {
    let text = |text: &String| cut_chars(text, MAX_WEB_TEXT_CHARS);
    let url = |url: &String| cut_chars(url, MAX_WEB_URL_CHARS);
    match action {
        HostedAction::Search { query } => (WebAction::Search, query.as_ref().map(text), None, None),
        HostedAction::Open { url: page } => (WebAction::Open, None, page.as_ref().map(url), None),
        HostedAction::Find { pattern, url: page } => {
            (WebAction::Find, None, page.as_ref().map(url), Some(text(pattern)))
        }
        // NOTE: an action of a later efr-provider; the client shows a plain web row.
        _ => (WebAction::Unknown, None, None, None),
    }
}

/// The results and the error of `outcome`, cut for an event.
fn outcome_parts(outcome: &HostedOutcome) -> (Option<Vec<WebResult>>, Option<WebError>) {
    match outcome {
        HostedOutcome::Done { results } => {
            let results = results.as_ref().map(|results| {
                results
                    .iter()
                    .take(MAX_WEB_RESULTS)
                    .map(|result| WebResult {
                        url: cut_chars(&result.url, MAX_WEB_URL_CHARS),
                        title: result
                            .title
                            .as_ref()
                            .map(|title| cut_chars(title, MAX_WEB_TITLE_CHARS)),
                    })
                    .collect()
            });
            (results, None)
        }
        HostedOutcome::Failed { error } => (None, Some(web_error(*error))),
        // NOTE: an outcome of a later efr-provider: the step did not say that it went well.
        _ => (None, Some(WebError::Unknown)),
    }
}

/// efr's wire word for `error`.
fn web_error(error: HostedError) -> WebError {
    match error {
        HostedError::TooManyRequests => WebError::TooManyRequests,
        HostedError::LimitReached => WebError::LimitReached,
        HostedError::Unavailable => WebError::Unavailable,
        HostedError::Invalid => WebError::Invalid,
        HostedError::NotAllowed => WebError::NotAllowed,
        HostedError::NotAccessible => WebError::NotAccessible,
        HostedError::Unsupported => WebError::Unsupported,
        HostedError::NotFinished => WebError::NotFinished,
        _ => WebError::Unknown,
    }
}

fn started(turn_id: TurnId, call_id: CallId, action: &HostedAction) -> Event {
    let (action, query, url, pattern) = action_parts(action);
    Event::WebCallStarted { turn_id, call_id, action, query, url, pattern }
}

fn completed(
    turn_id: TurnId,
    call_id: CallId,
    action: &HostedAction,
    results: Option<Vec<WebResult>>,
    error: Option<WebError>,
) -> Event {
    let (action, query, url, pattern) = action_parts(action);
    Event::WebCallCompleted { turn_id, call_id, action, query, url, pattern, results, error }
}

/// `text` cut to `max` characters.
fn cut_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text[..at].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests;
