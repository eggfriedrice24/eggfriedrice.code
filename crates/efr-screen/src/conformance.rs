//! The conformance suite every screen backend runs.
//!
//! [`run`] drives the real [`ScreenActor`] with a backend's factory over every fixture
//! in `fixtures/vt/` (bytes in, rows, cursor, title, bells and replies out) and
//! `fixtures/shell_marks/` (bytes in, marks with recording offsets out), so both
//! backends are tested through the identical actor code and only the factory changes.
//! The fixture format is described in `conformance/cases.rs`.
//!
//! Besides the expectations written in the fixtures, every `vt` fixture is recorded
//! per backend as an insta snapshot in `fixtures/vt/snapshots/<case>__<backend>.snap`,
//! so a change in what a backend renders shows up even where the fixture lists the
//! backend in `differs`.
//!
//! The suite is a test harness: it panics with a report of every failure, so a
//! backend's `tests/it/conformance.rs` is one call. A screen that blocks forever shows up
//! as a hang that nextest's slow-timeout ends, never as a sleep in the suite.

mod cases;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;

use bytes::Bytes;
use efr_protocol::{Seq, Size};
use serde_json::{Map, Value, json};

use self::cases::{Action, Case, Step};
use crate::{
    ClickMode, PromptKind, Screen, ScreenActor, ScreenCapture, ScreenEvent, ScreenEvents,
    ScreenHandle, SemanticPromptEvent, ShellMark, ShellMarkKind, row_text,
};

/// The fixture directory of this crate, fixed at compile time so a backend crate's
/// test finds it whatever its working directory.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");

/// Runs every fixture against the backend called `backend` (the name the fixtures'
/// `differs` lists use, such as `vt100` or `ghostty`), building each screen with
/// `factory(size)`.
///
/// # Panics
///
/// Panics with a report of every failed expectation, when a fixture cannot be read,
/// or when a `vt` fixture's rendering no longer matches its snapshot.
pub fn run<M, F, S>(backend: &str, factory: M)
where
    M: Fn(Size) -> F,
    F: FnOnce() -> S + Send + 'static,
    S: Screen + 'static,
{
    let root = Path::new(FIXTURES);
    let mut failures = Vec::new();
    let mut renderings = Vec::new();
    for dir in ["vt", "shell_marks"] {
        let cases = cases::load_dir(&root.join(dir)).unwrap_or_else(|err| fail(&err.to_string()));
        for case in cases {
            let outcome = drive(backend, &factory, &case);
            failures.extend(outcome.failures);
            if dir == "vt" {
                renderings.push((case, outcome.rendering));
            }
        }
    }
    if !failures.is_empty() {
        let mut report =
            format!("{} conformance failures for backend {backend}:\n", failures.len());
        for failure in &failures {
            let _ = writeln!(report, "{failure}");
        }
        fail(&report);
    }
    for (case, rendering) in renderings {
        assert_rendering(backend, &case, &rendering);
    }
}

#[expect(
    clippy::panic,
    reason = "the conformance suite is a test harness; a failure must fail the calling test"
)]
fn fail(report: &str) -> ! {
    panic!("{report}");
}

/// Compares a case's rendering with its snapshot for this backend.
fn assert_rendering(backend: &str, case: &Case, rendering: &str) {
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(Path::new(FIXTURES).join("vt/snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.set_input_file(&case.path);
    let name = format!("{}__{backend}", case.name);
    settings.bind(|| insta::assert_snapshot!(name.as_str(), rendering));
}

/// One expectation that did not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) check: &'static str,
    pub(crate) expected: String,
    pub(crate) actual: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = self.path.strip_prefix(FIXTURES).unwrap_or(&self.path);
        write!(
            f,
            "  {}:{} ({}): expected {}, got {}",
            path.display(),
            self.line,
            self.check,
            self.expected,
            self.actual
        )
    }
}

/// What driving one case produced.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub(crate) failures: Vec<Failure>,
    /// The final screen and everything the backend reported, for the snapshot.
    pub(crate) rendering: String,
}

/// Events seen since the matching expectation last looked.
#[derive(Debug, Default)]
struct Pending {
    marks: VecDeque<ShellMark>,
    replies: Vec<u8>,
    bells: usize,
    titles: Vec<String>,
}

/// Everything the backend reported over the whole case, for the rendering.
#[derive(Debug, Default)]
struct Totals {
    replies: Vec<u8>,
    bells: usize,
    titles: Vec<String>,
}

/// Runs one case on a fresh actor.
pub(crate) fn drive<M, F, S>(backend: &str, factory: &M, case: &Case) -> Outcome
where
    M: Fn(Size) -> F,
    F: FnOnce() -> S + Send + 'static,
    S: Screen + 'static,
{
    let mut driver = match Driver::start(backend, factory(case.size), case) {
        Ok(driver) => driver,
        Err(failure) => return Outcome { failures: vec![failure], rendering: String::new() },
    };
    for step in &case.steps {
        if let Err(failure) = driver.step(step) {
            driver.failures.push(failure);
            break;
        }
    }
    driver.finish()
}

struct Driver<'a> {
    case: &'a Case,
    /// True when this backend is in the case's `differs` list.
    differs: bool,
    handle: ScreenHandle,
    events: std_mpsc::Receiver<ScreenEvent>,
    next_seq: u64,
    barriers: u64,
    pending: Pending,
    totals: Totals,
    last: Option<ScreenCapture>,
    failures: Vec<Failure>,
}

impl<'a> Driver<'a> {
    fn start<F, S>(backend: &str, factory: F, case: &'a Case) -> Result<Self, Failure>
    where
        F: FnOnce() -> S + Send + 'static,
        S: Screen + 'static,
    {
        let not_started = |what: String| failure(case, 0, "start", "a running screen", what);
        let (handle, events) = ScreenActor::spawn("screen-conformance", factory, case.size)
            .map_err(|err| not_started(err.to_string()))?;
        // A separate reader keeps the event queue drained, so the driver can never
        // block on a full command queue while the actor blocks on a full event queue.
        let (forward, events_rx) = std_mpsc::channel();
        efr_stdx::thread::spawn_named("screen-events", 256 * 1024, move || {
            forward_events(events, &forward)
        })
        .map_err(|err| not_started(err.to_string()))?;
        Ok(Driver {
            case,
            differs: case.differs.iter().any(|name| name == backend),
            handle,
            events: events_rx,
            next_seq: 0,
            barriers: 0,
            pending: Pending::default(),
            totals: Totals::default(),
            last: None,
            failures: Vec::new(),
        })
    }

    fn step(&mut self, step: &Step) -> Result<(), Failure> {
        let line = step.line;
        match &step.action {
            Action::Feed { chunks, seq } => {
                if let Some(seq) = seq {
                    self.next_seq = *seq;
                }
                for chunk in chunks {
                    self.handle
                        .feed_blocking(Bytes::copy_from_slice(chunk), Seq::new(self.next_seq))
                        .map_err(|err| self.stopped(line, &err.to_string()))?;
                    self.next_seq += chunk.len() as u64;
                }
            }
            Action::Resize(size) => {
                self.handle
                    .resize_blocking(*size)
                    .map_err(|err| self.stopped(line, &err.to_string()))?;
            }
            expectation => {
                let capture = self.barrier(line)?;
                self.check(line, expectation, &capture);
                self.last = Some(capture);
            }
        }
        Ok(())
    }

    /// Waits until the actor has handled every command so far, collecting the events
    /// they produced, and returns a snapshot taken at that point.
    fn barrier(&mut self, line: usize) -> Result<ScreenCapture, Failure> {
        self.barriers += 1;
        let id = self.barriers;
        self.handle
            .request_snapshot_blocking(id, 0)
            .map_err(|err| self.stopped(line, &err.to_string()))?;
        loop {
            match self.events.recv() {
                Ok(ScreenEvent::Snapshot { id: got, capture }) if got == id => return Ok(capture),
                Ok(event) => self.collect(event),
                Err(_) => return Err(self.stopped(line, "the event stream ended")),
            }
        }
    }

    fn collect(&mut self, event: ScreenEvent) {
        match event {
            ScreenEvent::PtyReply(bytes) => {
                self.pending.replies.extend_from_slice(&bytes);
                self.totals.replies.extend_from_slice(&bytes);
            }
            ScreenEvent::ShellMark(mark) => self.pending.marks.push_back(mark),
            ScreenEvent::TitleChanged(title) => {
                self.pending.titles.push(title.clone());
                self.totals.titles.push(title);
            }
            ScreenEvent::Bell => {
                self.pending.bells += 1;
                self.totals.bells += 1;
            }
            ScreenEvent::Snapshot { .. } => {}
        }
    }

    fn check(&mut self, line: usize, expectation: &Action, capture: &ScreenCapture) {
        match expectation {
            Action::ExpectMarks(expected) => {
                let actual: Vec<Value> =
                    self.pending.marks.drain(..).map(|mark| mark_json(&mark)).collect();
                self.compare(
                    line,
                    "marks",
                    false,
                    &Value::Array(expected.clone()),
                    &Value::Array(actual),
                );
            }
            Action::ExpectRows(expected) => {
                let actual: Vec<String> = capture.snapshot.rows.iter().map(row_text).collect();
                self.compare(line, "rows", true, expected, &actual);
            }
            Action::ExpectCursor { row, col } => {
                let cursor = capture.snapshot.cursor;
                self.compare(line, "cursor", true, &(*row, *col), &(cursor.row, cursor.col));
            }
            Action::ExpectSize(size) => {
                self.compare(line, "size", true, size, &capture.snapshot.size);
            }
            Action::ExpectTitle { title, changes } => {
                self.compare(line, "title", true, title, &capture.snapshot.title);
                let seen = std::mem::take(&mut self.pending.titles);
                if let Some(changes) = changes {
                    self.compare(line, "title changes", true, changes, &seen);
                }
            }
            Action::ExpectBells(count) => {
                let seen = std::mem::take(&mut self.pending.bells);
                self.compare(line, "bells", true, count, &seen);
            }
            Action::ExpectReplies(expected) => {
                let seen = std::mem::take(&mut self.pending.replies);
                self.compare(line, "replies", true, &Escaped(expected), &Escaped(&seen));
            }
            Action::Feed { .. } | Action::Resize(_) => {}
        }
    }

    /// Records a failure when `expected` and `actual` differ. Backend-dependent checks
    /// are skipped for a backend the case lists in `differs`.
    fn compare<T: PartialEq + std::fmt::Debug + ?Sized>(
        &mut self,
        line: usize,
        check: &'static str,
        backend_dependent: bool,
        expected: &T,
        actual: &T,
    ) {
        if (backend_dependent && self.differs) || expected == actual {
            return;
        }
        let failure =
            failure(self.case, line, check, format!("{expected:?}"), format!("{actual:?}"));
        self.failures.push(failure);
    }

    /// Flushes the case: everything must have been expected, then the actor must stop
    /// and its event stream end.
    fn finish(mut self) -> Outcome {
        let end = self.case.steps.last().map_or(1, |step| step.line);
        if self.failures.is_empty() {
            match self.barrier(end) {
                Ok(capture) => {
                    self.leftovers(end);
                    self.last = Some(capture);
                }
                Err(failure) => self.failures.push(failure),
            }
        }
        if self.handle.shutdown_blocking().is_ok() {
            // The forwarding thread ends when the actor's event stream ends.
            while let Ok(event) = self.events.recv() {
                self.collect(event);
            }
        }
        let rendering = render(self.case, self.last.as_ref(), &self.totals);
        Outcome { failures: self.failures, rendering }
    }

    fn leftovers(&mut self, line: usize) {
        let marks: Vec<Value> = self.pending.marks.drain(..).map(|mark| mark_json(&mark)).collect();
        self.compare(
            line,
            "unexpected marks at the end",
            false,
            &Value::Array(Vec::new()),
            &Value::Array(marks),
        );
        let replies = std::mem::take(&mut self.pending.replies);
        self.compare(
            line,
            "unexpected replies at the end",
            true,
            &Escaped(&[]),
            &Escaped(&replies),
        );
        let bells = std::mem::take(&mut self.pending.bells);
        self.compare(line, "unexpected bells at the end", true, &0, &bells);
    }

    fn stopped(&self, line: usize, what: &str) -> Failure {
        failure(self.case, line, "screen", "a running screen", what.to_owned())
    }
}

fn failure(
    case: &Case,
    line: usize,
    check: &'static str,
    expected: impl Into<String>,
    actual: String,
) -> Failure {
    Failure { path: case.path.clone(), line, check, expected: expected.into(), actual }
}

fn forward_events(mut events: ScreenEvents, forward: &std_mpsc::Sender<ScreenEvent>) {
    while let Some(event) = events.blocking_recv() {
        if forward.send(event).is_err() {
            break;
        }
    }
}

/// The fixture form of a mark: `{"start":S,"end":E,"<kind>":{...}}`, with absent
/// options left out.
pub(crate) fn mark_json(mark: &ShellMark) -> Value {
    let (kind, fields) = match &mark.kind {
        ShellMarkKind::SemanticPrompt(event) => semantic_prompt_json(event),
        ShellMarkKind::CwdChanged { host, path } => {
            let mut fields = Map::new();
            insert_some(&mut fields, "host", host.as_deref());
            fields.insert("path".to_owned(), json!(path.to_string_lossy()));
            ("cwd_changed", fields)
        }
    };
    let mut object = Map::new();
    object.insert("start".to_owned(), json!(mark.start.get()));
    object.insert("end".to_owned(), json!(mark.end.get()));
    object.insert(kind.to_owned(), Value::Object(fields));
    Value::Object(object)
}

fn semantic_prompt_json(event: &SemanticPromptEvent) -> (&'static str, Map<String, Value>) {
    let mut fields = Map::new();
    let kind = match event {
        SemanticPromptEvent::PromptStart { kind, aid, click, fresh_line } => {
            fields.insert("kind".to_owned(), json!(prompt_kind_name(*kind)));
            fields.insert("fresh_line".to_owned(), json!(fresh_line));
            insert_some(&mut fields, "aid", aid.as_deref());
            insert_some(&mut fields, "click", click.map(click_mode_name));
            "prompt_start"
        }
        SemanticPromptEvent::InputStart => "input_start",
        SemanticPromptEvent::OutputStart { command, aid } => {
            insert_some(&mut fields, "command", command.as_deref());
            insert_some(&mut fields, "aid", aid.as_deref());
            "output_start"
        }
        SemanticPromptEvent::CommandEnd { exit_code, error, aid } => {
            if let Some(code) = exit_code {
                fields.insert("exit_code".to_owned(), json!(code));
            }
            insert_some(&mut fields, "error", error.as_deref());
            insert_some(&mut fields, "aid", aid.as_deref());
            "command_end"
        }
    };
    (kind, fields)
}

fn insert_some(fields: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        fields.insert(key.to_owned(), json!(value));
    }
}

fn prompt_kind_name(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::Initial => "initial",
        PromptKind::Continuation => "continuation",
        PromptKind::Secondary => "secondary",
        PromptKind::Right => "right",
    }
}

fn click_mode_name(click: ClickMode) -> &'static str {
    match click {
        ClickMode::Line => "line",
        ClickMode::Multiple => "multiple",
        ClickMode::ConservativeVertical => "conservative_vertical",
        ClickMode::SmartVertical => "smart_vertical",
    }
}

/// Bytes as printable ASCII with every other byte escaped, for readable reports.
fn escape(bytes: &[u8]) -> String {
    bytes.escape_ascii().to_string()
}

/// Reply bytes that print as one quoted, escaped string in a failure report.
#[derive(PartialEq, Eq)]
struct Escaped<'a>(&'a [u8]);

impl std::fmt::Debug for Escaped<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"{}\"", self.0.escape_ascii())
    }
}

/// The snapshot text of a case: the final screen and what the backend reported.
fn render(case: &Case, last: Option<&ScreenCapture>, totals: &Totals) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "case: {}", case.name);
    match last {
        Some(capture) => {
            let snapshot = &capture.snapshot;
            let _ = writeln!(out, "size: {}x{}", snapshot.size.cols, snapshot.size.rows);
            let _ =
                writeln!(out, "cursor: row {}, col {}", snapshot.cursor.row, snapshot.cursor.col);
            let _ = writeln!(out, "title: {}", snapshot.title.as_deref().unwrap_or("(none)"));
            let _ = writeln!(out, "rows:");
            for (index, row) in snapshot.rows.iter().enumerate() {
                let _ = writeln!(out, "{index:>3} |{}|", row_text(row));
            }
        }
        None => {
            let _ = writeln!(out, "no snapshot: the screen stopped");
        }
    }
    let _ = writeln!(out, "title changes: {:?}", totals.titles);
    let _ = writeln!(out, "bells: {}", totals.bells);
    let _ = write!(out, "replies: \"{}\"", escape(&totals.replies));
    out
}

#[cfg(test)]
mod tests;
