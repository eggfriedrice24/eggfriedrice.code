//! The rows of web search (TM-Web, `DECISIONS-web.md` 3): the provider runs the search
//! inside the model call, and efrd records its steps (`web_call_started`,
//! `web_call_completed`) and what the turn did on the web (`turn_web_report`). The
//! follow view (`follow::view::web`) and `efr history` write the same rows from them.
//!
//! ```text
//! ✓ web "ROB RUN" Proton Easy Anti-Cheat  10 results · 5s
//! ✓ web "ROB RUN" areweanticheatyet  5s
//! ✓ web open areweanticheatyet.com/game/rob-run  2s
//! ! web "ROB RUN" 1.4 patch notes Linux  too many requests · 1s
//!
//! [1] https://areweanticheatyet.com/game/rob-run  ROB RUN · Are We Anti-Cheat Yet?
//! ```
//!
//! - A search: `✓` in `success`, `web` and the query muted, the query as the model sent
//!   it, with its own quotes. Two spaces, then the count of results when efr knows it
//!   (`0 results` is said) and the time, muted. A long query is cut after the last whole
//!   word that fits, then `…`; the count and the time are never cut. A search without a
//!   query is `web search`.
//! - An open (OpenAI): `web open` and the page without `https://` and `www.`, in the
//!   `link` role with an OSC 8 link to the whole URL; a page that does not fit is cut in
//!   the middle, `store.steampowered.com/…/ROB_RUN/`. A find in a page has no row.
//! - A failed step: `!` and the reason in `warning`, the rest muted, no tail. A failure
//!   shows its time from a tenth of a second on, as a call that went well does.
//! - The list of sources after the answer: `[n]` muted, the whole URL in the `link`
//!   role, then the title muted after two spaces. The title is cut, then left out, before
//!   the URL is touched: efr never cuts or breaks a URL, the terminal wraps it.
//! - The end line counts the searches that ran, `· 5 searches`, and says `stored copy`
//!   for a turn that read the provider's stored copy of the web; the first such turn of
//!   a conversation gets one muted row under it that says what it means.
//!
//! Every glyph comes from the options (`Glyphs`), so a locale that is not UTF-8 gets the
//! ASCII form of each mark.

use std::fmt::Write as _;
use std::time::Duration;

use efr_protocol::{CallId, Event, WebAction, WebError, WebResult, WebSource};
use efr_render::{Glyphs, RenderOptions, WidthMethod, text_width};

use super::{Tone, columns, fits, one_line, one_line_words, paint, pieces, wrap_spans};

/// The word of every row of web search, muted, as `read` and `edit` are.
pub(crate) const WORD: &str = "web";

/// The word after [`WORD`] in the row of an opened page.
const OPEN: &str = "open";

/// The word after [`WORD`] in the row of a search whose query efr never got.
const SEARCH: &str = "search";

/// Between the subject of a row and its count and time.
const GAP: &str = "  ";

/// How many columns after the column of the mark a row that goes on starts, as the
/// rows of a command do (`follow::view::call`).
const GOES_ON: usize = 4;

/// The columns that a cut title of a source keeps at least; with fewer, the row has no
/// title (TM-Web 12: 12 columns left drop it).
const MIN_TITLE: usize = 16;

/// The results that `efr history --verbose` lists under a search; the rest are counted.
pub(crate) const SHOWN_RESULTS: usize = 3;

/// The time from which a row says how long its step ran, as the row of a call does: a
/// shorter one would read `0.0s`.
const ROW_TIME: Duration = Duration::from_millis(100);

/// One step of the web tool as the rows name it: what it does and its subject. A
/// `web_call_completed` carries the whole step again, so it fills what its start lacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) action: WebAction,
    /// The query of a search, as the model sent it.
    pub(crate) query: Option<String>,
    /// The page of an open or a find.
    pub(crate) url: Option<String>,
    /// The text that a find looks for.
    pub(crate) pattern: Option<String>,
}

impl Step {
    /// The call and its step of a `web_call_started` or a `web_call_completed`; `None`
    /// for any other event.
    pub(crate) fn of(event: &Event) -> Option<(CallId, Step)> {
        let (call_id, action, query, url, pattern) = match event {
            Event::WebCallStarted { call_id, action, query, url, pattern, .. }
            | Event::WebCallCompleted { call_id, action, query, url, pattern, .. } => {
                (call_id, action, query, url, pattern)
            }
            _ => return None,
        };
        let step = Step {
            action: *action,
            query: query.clone(),
            url: url.clone(),
            pattern: pattern.clone(),
        };
        Some((*call_id, step))
    }

    /// The same step with what `later` knows: the end of a step names its whole state.
    #[must_use]
    pub(crate) fn with(mut self, later: Step) -> Step {
        if later.action != WebAction::Unknown || self.action == WebAction::Unknown {
            self.action = later.action;
        }
        self.query = later.query.or(self.query);
        self.url = later.url.or(self.url);
        self.pattern = later.pattern.or(self.pattern);
        self
    }

    /// True for a step that never gets a row: a find in a page. Its text shows in the
    /// status row while it runs and in `efr history --verbose` under its open.
    pub(crate) fn rowless(&self) -> bool {
        self.action == WebAction::Find
    }

    /// True for the opening of a page: the status row says `opening`.
    pub(crate) fn opens(&self) -> bool {
        self.action == WebAction::Open
    }

    /// What the status row names while the step runs, safe to print: the query of a
    /// search, the page of an open as `host/path`, `"<pattern>" in <host>` for a find;
    /// `None` when efr does not know it yet.
    pub(crate) fn detail(&self) -> Option<String> {
        match self.action {
            WebAction::Search => {
                self.query.as_deref().filter(|query| !query.trim().is_empty()).map(one_line_words)
            }
            WebAction::Open => self.url.as_deref().map(page),
            WebAction::Find => {
                let pattern = self.pattern.as_deref().map(one_line_words)?;
                Some(match self.url.as_deref() {
                    Some(url) => format!("\"{pattern}\" in {}", host(url)),
                    None => format!("\"{pattern}\""),
                })
            }
            _ => None,
        }
    }
}

/// How a step of the web tool ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum End {
    /// It went well; a search with the count of its results when the provider gave them.
    Done { results: Option<usize> },
    /// It failed for this reason.
    Failed(WebError),
}

impl End {
    /// How the step of a `web_call_completed` ended; `None` for any other event.
    pub(crate) fn of(event: &Event) -> Option<End> {
        match event {
            Event::WebCallCompleted { error: Some(error), .. } => Some(End::Failed(*error)),
            Event::WebCallCompleted { results, .. } => {
                Some(End::Done { results: results.as_ref().map(Vec::len) })
            }
            _ => None,
        }
    }
}

/// A step that ended, with how long it ran when efr saw its start: what its row needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ended {
    pub(crate) call_id: CallId,
    pub(crate) step: Step,
    pub(crate) end: End,
    pub(crate) took: Option<Duration>,
}

/// How a row of a search treats a query that does not fit the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fit {
    /// The row of the turn: one row, the query cut after the last whole word that fits.
    Cut,
    /// `efr history`: the whole query, which goes on in the next rows as a command does.
    Wrap,
}

/// The reason of a failed step in words (TM-Web 8 for a search): `too many requests`,
/// `search limit of this turn reached`, `web search not available`.
pub(crate) fn reason(action: WebAction, error: WebError) -> &'static str {
    let open = action == WebAction::Open;
    match error {
        WebError::TooManyRequests => "too many requests",
        WebError::LimitReached if open => "page limit of this turn reached",
        WebError::LimitReached => "search limit of this turn reached",
        WebError::Unavailable if open => "page not available",
        WebError::Unavailable => "web search not available",
        WebError::Invalid if open => "address not accepted",
        WebError::Invalid => "query not accepted",
        WebError::NotAllowed => "page not allowed",
        WebError::NotAccessible => "page not reachable",
        WebError::Unsupported => "page type not supported",
        WebError::NotFinished => "did not finish",
        _ => "failed",
    }
}

/// The row of a step that ended, with its newline; a find has none. `fit` says what a
/// query wider than the screen does. Off a terminal the row is one plain row, whole.
pub(crate) fn row(ended: &Ended, fit: Fit, options: &RenderOptions) -> String {
    if ended.step.rowless() {
        return String::new();
    }
    let glyphs = options.glyphs();
    let method = options.width_method();
    let (mark, marked) = match ended.end {
        End::Done { .. } => (glyphs.ok, Tone::Success),
        End::Failed(_) => (glyphs.refused, Tone::Attention),
    };
    let suffix = suffix(ended, glyphs);
    let suffix_width: usize = suffix.iter().map(|(text, _)| text_width(text, method)).sum();
    let column = text_width(mark, method) + 1;
    let mut out = paint(mark, marked, options);
    out.push(' ');
    let width = columns(options).map(|(width, _)| width);
    if ended.step.opens() {
        out.push_str(&paint(&format!("{WORD} {OPEN}"), Tone::Dim, options));
        let Some(url) = ended.step.url.as_deref().filter(|url| !url.trim().is_empty()) else {
            push_pieces(&mut out, &suffix, options);
            out.push('\n');
            return out;
        };
        let used = column + text_width(WORD, method) + 1 + text_width(OPEN, method) + 1;
        out.push(' ');
        match (width, fit) {
            (Some(width), Fit::Cut) => {
                let room = width.saturating_sub(used + suffix_width);
                let shown = cut_page(&page(url), room, glyphs, method);
                out.push_str(&linked(&paint(&shown, Tone::Link, options), url, options));
            }
            // NOTE: `efr history` keeps the whole page, at efr's indent as a long query,
            // so the terminal never wraps it at column 0; each piece links the URL.
            (Some(width), Fit::Wrap) => {
                let rest = width.saturating_sub(column + GOES_ON).max(1);
                let parts = chunks(&page(url), width.saturating_sub(used).max(1), rest, method);
                for (at, part) in parts.iter().enumerate() {
                    if at > 0 {
                        out.push('\n');
                        out.push_str(&" ".repeat(column + GOES_ON));
                    }
                    out.push_str(&linked(&paint(part, Tone::Link, options), url, options));
                }
                let lead = if parts.len() == 1 { used } else { column + GOES_ON };
                let used = lead + parts.last().map_or(0, |part| text_width(part, method));
                if !suffix.is_empty() && used + suffix_width > width {
                    out.push('\n');
                    out.push_str(&" ".repeat(column + GOES_ON));
                    push_pieces(&mut out, after_gap(&suffix), options);
                } else {
                    push_pieces(&mut out, &suffix, options);
                }
                out.push('\n');
                return out;
            }
            (None, _) => {
                out.push_str(&linked(&paint(&page(url), Tone::Link, options), url, options));
            }
        }
        push_pieces(&mut out, &suffix, options);
        out.push('\n');
        return out;
    }
    // NOTE: a blank query names nothing, so the row says `web search` as for none.
    let query = ended.step.query.as_deref().filter(|query| !query.trim().is_empty());
    let subject = match (&ended.step.action, query) {
        (_, Some(query)) => format!("{WORD} {}", one_line_words(query)),
        (WebAction::Search, None) => format!("{WORD} {SEARCH}"),
        _ => WORD.to_owned(),
    };
    match (width, fit) {
        (Some(width), Fit::Cut) => {
            let room = width.saturating_sub(column + suffix_width);
            let word = text_width(WORD, method) + 1;
            let shown = match subject.get(WORD.len() + 1..) {
                Some(query) if !fits(&subject, room, method) => {
                    format!(
                        "{WORD} {}",
                        cut_words(query, room.saturating_sub(word), glyphs, method)
                    )
                }
                _ => subject,
            };
            out.push_str(&paint(&shown, Tone::Dim, options));
            push_pieces(&mut out, &suffix, options);
            out.push('\n');
        }
        // NOTE: only the query wraps; the count, the time and the reason stay whole on
        // the last row, or on a row of their own when they do not fit after it
        // (DECISIONS-web 3: they are never cut).
        (Some(width), Fit::Wrap) => {
            let first = width.saturating_sub(column).max(1);
            let rest = width.saturating_sub(column + GOES_ON).max(1);
            let mut rows = wrap_spans(&[(subject, Tone::Dim)], first, rest, method);
            if !suffix.is_empty() {
                let room = if rows.len() == 1 { first } else { rest };
                let used: usize = rows
                    .last()
                    .map(|row| row.iter().map(|(text, _)| text_width(text, method)).sum())
                    .unwrap_or_default();
                match rows.last_mut() {
                    Some(last) if used + suffix_width <= room => last.extend(suffix),
                    _ => rows.push(after_gap(&suffix).to_vec()),
                }
            }
            for (at, row) in rows.iter().enumerate() {
                if at > 0 {
                    out.push_str(&" ".repeat(column + GOES_ON));
                }
                push_pieces(&mut out, row, options);
                out.push('\n');
            }
        }
        (None, _) => {
            out.push_str(&paint(&subject, Tone::Dim, options));
            push_pieces(&mut out, &suffix, options);
            out.push('\n');
        }
    }
    out
}

/// What follows the subject of a row: two spaces, then the count of results and the
/// time, muted, or the reason of a failure in `warning` and its time.
fn suffix(ended: &Ended, glyphs: &Glyphs) -> Vec<(String, Tone)> {
    let time = ended.took.filter(|took| *took >= ROW_TIME).map(took_text);
    let separator = glyphs.separator;
    match &ended.end {
        End::Done { results } => {
            let mut parts: Vec<String> = Vec::new();
            if ended.step.action == WebAction::Search
                && let Some(count) = results
            {
                parts.push(if *count == 1 {
                    "1 result".to_owned()
                } else {
                    format!("{count} results")
                });
            }
            parts.extend(time);
            if parts.is_empty() {
                return Vec::new();
            }
            vec![(GAP.to_owned(), Tone::Plain), (parts.join(&format!(" {separator} ")), Tone::Dim)]
        }
        End::Failed(error) => {
            let mut pieces = vec![
                (GAP.to_owned(), Tone::Plain),
                (reason(ended.step.action, *error).to_owned(), Tone::Attention),
            ];
            if let Some(time) = time {
                pieces.push((" ".to_owned(), Tone::Plain));
                pieces.push((format!("{separator} {time}"), Tone::Dim));
            }
            pieces
        }
    }
}

/// How long a step ran, as the row of a call says it: tenths of a second under 10 s,
/// without `.0` from 1 s on (`0.4s`, `6s`), else as the status row counts.
fn took_text(took: Duration) -> String {
    let text = super::took(took);
    match text.strip_suffix(".0s") {
        Some(whole) if whole != "0" => format!("{whole}s"),
        _ => text,
    }
}

/// `query` in `room` columns: whole when it fits, else cut after the last whole word
/// that fits with the mark of more text after it, or inside its first word when no word
/// fits.
fn cut_words(query: &str, room: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    if fits(query, room, method) {
        return query.to_owned();
    }
    let more = glyphs.more;
    let budget = room.saturating_sub(text_width(more, method));
    let words = query
        .match_indices(' ')
        .rev()
        .map(|(at, _)| query[..at].trim_end())
        .find(|kept| !kept.is_empty() && fits(kept, budget, method));
    match words {
        Some(kept) => format!("{kept}{more}"),
        None => cut_to(query, room, glyphs, method),
    }
}

/// `text` cut to `columns` with the mark of more text of `glyphs` at the cut; whole
/// when it fits. A grapheme cluster is never cut in two.
pub(crate) fn cut_to(text: &str, columns: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    if fits(text, columns, method) {
        return text.to_owned();
    }
    let more = glyphs.more;
    let budget = columns.saturating_sub(text_width(more, method));
    let mut out = String::new();
    let mut used = 0;
    for piece in pieces(text, method) {
        let width = text_width(piece, method);
        if used + width > budget {
            break;
        }
        out.push_str(piece);
        used += width;
    }
    out.push_str(more);
    out
}

/// The page of `url` as a row names it, safe to print: without the scheme and `www.`,
/// and without a `/` that ends a bare host: `areweanticheatyet.com/game/rob-run`.
pub(crate) fn page(url: &str) -> String {
    let url = one_line(url.trim());
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            url.get(..scheme.len())
                .filter(|start| start.eq_ignore_ascii_case(scheme))
                .map(|_| &url[scheme.len()..])
        })
        .unwrap_or(&url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.split_once('/') {
        Some((host, "")) => host.to_owned(),
        _ => rest.to_owned(),
    }
}

/// The host of `url` without `www.`, as the status row names the page of a find.
pub(crate) fn host(url: &str) -> String {
    let page = page(url);
    let end = page.find(['/', '?', '#']).unwrap_or(page.len());
    page[..end].to_owned()
}

/// `page` in `room` columns: whole when it fits, else cut in the middle, the host, `/…/`
/// and the longest end of its path that fits (`store.steampowered.com/…/ROB_RUN/`), else
/// cut at its end. A query string or a fragment goes first: it shows as `?…` (`#…`)
/// after the path, which says more of the page.
fn cut_page(page: &str, room: usize, glyphs: &Glyphs, method: WidthMethod) -> String {
    if fits(page, room, method) {
        return page.to_owned();
    }
    let more = glyphs.more;
    let (base, tail) = match page.find(['?', '#']) {
        Some(at) => (&page[..at], format!("{}{more}", &page[at..=at])),
        None => (page, String::new()),
    };
    if !tail.is_empty() {
        let shown = format!("{base}{tail}");
        if fits(&shown, room, method) {
            return shown;
        }
    }
    if let Some((host, path)) = base.split_once('/') {
        // NOTE: each end of the path from a `/` on, longest first, so the cut keeps as
        // much of the path as the row has room for.
        let ends = path.match_indices('/').map(|(at, _)| &path[at + 1..]);
        for end in ends.filter(|end| !end.is_empty()) {
            let shown = format!("{host}/{more}/{end}{tail}");
            if fits(&shown, room, method) {
                return shown;
            }
        }
    }
    cut_to(page, room, glyphs, method)
}

/// `painted`, the painted text of a page, as an OSC 8 link to `url` when the output is a
/// terminal with links on and `url` is a web address; else unchanged.
fn linked(painted: &str, url: &str, options: &RenderOptions) -> String {
    let url = url.trim();
    let web = ["https://", "http://"].iter().any(|scheme| {
        url.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if !web || !options.is_terminal() || !options.hyperlinks() {
        return painted.to_owned();
    }
    format!("\x1b]8;;{}\x1b\\{painted}\x1b]8;;\x1b\\", encode(url))
}

/// `url` with every byte outside printable ASCII percent-encoded, so nothing in it can
/// end the OSC 8 sequence early or start another one.
fn encode(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for byte in url.bytes() {
        if (0x21..0x7f).contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// `suffix` without the gap that starts it, for a row of its own.
fn after_gap(suffix: &[(String, Tone)]) -> &[(String, Tone)] {
    match suffix.split_first() {
        Some(((gap, _), rest)) if gap == GAP => rest,
        _ => suffix,
    }
}

/// `text` in rows of `first` columns, then `rest` columns each, cut between grapheme
/// clusters: a page has no space to cut at. Nothing of the text is left out.
fn chunks(text: &str, first: usize, rest: usize, method: WidthMethod) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    let mut room = first;
    for piece in pieces(text, method) {
        let width = text_width(piece, method);
        if used > 0 && used + width > room {
            rows.push(String::new());
            used = 0;
            room = rest;
        }
        if let Some(row) = rows.last_mut() {
            row.push_str(piece);
        }
        used += width;
    }
    rows
}

/// Paints `pieces` onto `out`.
fn push_pieces(out: &mut String, pieces: &[(String, Tone)], options: &RenderOptions) {
    for (text, tone) in pieces {
        out.push_str(&paint(text, *tone, options));
    }
}

/// The list of the sources of an answer (TM-Web 10 to 12), one row each with its
/// newline: `[1] https://areweanticheatyet.com/game/rob-run  ROB RUN · Are We Anti-Cheat
/// Yet?`. The number muted, the URL whole in the `link` role (an OSC 8 link too), the
/// title muted after two spaces: cut to the room the row leaves, and left out when
/// fewer than [`MIN_TITLE`] columns are left.
pub(crate) fn sources(sources: &[WebSource], options: &RenderOptions) -> String {
    let mut out = String::new();
    for (at, source) in sources.iter().enumerate() {
        let number = format!("[{}]", at + 1);
        let url = one_line(source.url.trim());
        let lead = format!("{number} {url}");
        out.push_str(&paint(&number, Tone::Dim, options));
        out.push(' ');
        out.push_str(&linked(&paint(&url, Tone::Link, options), &source.url, options));
        if let Some(title) = title(source.title.as_deref(), &lead, options) {
            out.push_str(GAP);
            out.push_str(&paint(&title, Tone::Dim, options));
        }
        out.push('\n');
    }
    out
}

/// The title of a page after `lead`, the rest of its row, safe to print and cut to the
/// room that the row leaves after two spaces; `None` when there is no title or too
/// little room for one. Off a terminal it is whole.
fn title(title: Option<&str>, lead: &str, options: &RenderOptions) -> Option<String> {
    let title = one_line_words(title?.trim());
    if title.is_empty() {
        return None;
    }
    let Some((width, method)) = columns(options) else {
        return Some(title);
    };
    let room = width.saturating_sub(text_width(lead, method) + GAP.len());
    if fits(&title, room, method) {
        return Some(title);
    }
    (room >= MIN_TITLE).then(|| cut_to(&title, room, options.glyphs(), method))
}

/// The count of the searches of a turn for the end of its cost line: `5 searches`, `1
/// search`, with `, stored copy` when they read the provider's stored copy of the web;
/// `None` without a search.
pub(crate) fn count(searches: u32, stored_copy: bool) -> Option<String> {
    let mut text = match searches {
        0 => return None,
        1 => "1 search".to_owned(),
        many => format!("{many} searches"),
    };
    if stored_copy {
        text.push_str(", stored copy");
    }
    Some(text)
}

/// `line`, an outcome line of the end of a turn, with the count of its searches last on
/// its cost (TM-Web 13): `✓ done  12s · ctx 9% · 640 out · 1 search`.
pub(crate) fn add_count(line: &mut Vec<(String, Tone)>, count: &str, glyphs: &Glyphs) {
    // NOTE: the cost is the one muted piece after the two spaces (`format::ending`).
    let costed = line.len() > 3;
    match line.last_mut() {
        Some((cost, Tone::Dim)) if costed => {
            let _ = write!(cost, " {} {count}", glyphs.separator);
        }
        _ => {
            line.push((GAP.to_owned(), Tone::Plain));
            line.push((count.to_owned(), Tone::Dim));
        }
    }
}

/// The muted row under the end line of the first turn of a conversation that read the
/// provider's stored copy of the web (TM-Web 13).
pub(crate) fn stored_copy_note(glyphs: &Glyphs) -> String {
    format!(
        "a stored copy of the web can be some days old {} ,mode auto searches the live web",
        glyphs.separator
    )
}

/// The rows that `efr history --verbose` puts under a search (TM-Web 15), each after
/// `indent`: the first [`SHOWN_RESULTS`] results as `<title>  <URL>`, the title muted and
/// the URL in the `link` role, then `… 5 more · cited: 1, 3`, muted. `sources` are the
/// pages that the answer of the turn cites: the numbers are those among `results`.
pub(crate) fn results_rows(
    results: &[WebResult],
    sources: &[WebSource],
    indent: &str,
    options: &RenderOptions,
) -> String {
    let glyphs = options.glyphs();
    let mut out = String::new();
    for result in results.iter().take(SHOWN_RESULTS) {
        let url = one_line(result.url.trim());
        out.push_str(indent);
        let lead = format!("{indent}{url}");
        if let Some(title) = title(result.title.as_deref(), &lead, options) {
            out.push_str(&paint(&title, Tone::Dim, options));
            out.push_str(GAP);
        }
        out.push_str(&linked(&paint(&url, Tone::Link, options), &result.url, options));
        out.push('\n');
    }
    let more = results.len().saturating_sub(SHOWN_RESULTS);
    let cited: Vec<String> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| results.iter().any(|result| result.url == source.url))
        .map(|(at, _)| (at + 1).to_string())
        .collect();
    let mut tail: Vec<String> = Vec::new();
    if more > 0 {
        tail.push(format!("{} {more} more", glyphs.more));
    }
    if !cited.is_empty() {
        tail.push(format!("cited: {}", cited.join(", ")));
    }
    if !tail.is_empty() {
        let line = tail.join(&format!(" {} ", glyphs.separator));
        let _ = writeln!(out, "{indent}{}", paint(&line, Tone::Dim, options));
    }
    out
}

/// The row that `efr history --verbose` puts under an open (TM-Web 15): its finds, `find
/// "Easy Anti-Cheat" · find "Proton"`, muted, after `indent`; empty without a find.
pub(crate) fn finds_row(patterns: &[String], indent: &str, options: &RenderOptions) -> String {
    if patterns.is_empty() {
        return String::new();
    }
    let separator = format!(" {} ", options.glyphs().separator);
    let line = patterns
        .iter()
        .map(|pattern| format!("find \"{}\"", one_line_words(pattern)))
        .collect::<Vec<_>>()
        .join(&separator);
    format!("{indent}{}\n", paint(&line, Tone::Dim, options))
}

/// The rows of web steps in the order in which they started (TM-Web 4): a row is
/// written when its step ended and every step that started before it has its row, so
/// the scrollback reads in the model's order. A find has no row, so it holds back none.
#[derive(Debug, Default)]
pub(crate) struct Order {
    /// The steps with a row, in the order of their starts, each with its end once it
    /// came.
    waiting: Vec<(CallId, Option<Ended>)>,
}

impl Order {
    /// Step `call_id` started; a step that never gets a row is not kept.
    pub(crate) fn start(&mut self, call_id: CallId, step: &Step) {
        if !step.rowless() && !self.waiting.iter().any(|(id, _)| *id == call_id) {
            self.waiting.push((call_id, None));
        }
    }

    /// A step ended: the rows that can be written now, in order. A step whose start
    /// was not seen counts as one that started now. NOTE: a step can end as a find
    /// although it started without its action (OpenAI sends the action only with the
    /// end of the step); the rows after it then wait for it no more.
    pub(crate) fn end(&mut self, ended: Ended) -> Vec<Ended> {
        if ended.step.rowless() {
            self.waiting.retain(|(id, _)| *id != ended.call_id);
            return self.ready();
        }
        match self.waiting.iter_mut().find(|(id, _)| *id == ended.call_id) {
            Some((_, slot)) => *slot = Some(ended),
            None => self.waiting.push((ended.call_id, Some(ended))),
        }
        self.ready()
    }

    /// The rows of the steps that ended, also those that wait for a step that never
    /// ended, in order; nothing waits after it.
    pub(crate) fn rest(&mut self) -> Vec<Ended> {
        std::mem::take(&mut self.waiting).into_iter().filter_map(|(_, ended)| ended).collect()
    }

    /// The ended steps at the start of the queue.
    fn ready(&mut self) -> Vec<Ended> {
        let count = self.waiting.iter().take_while(|(_, ended)| ended.is_some()).count();
        self.waiting.drain(..count).filter_map(|(_, ended)| ended).collect()
    }
}

#[cfg(test)]
mod tests;
