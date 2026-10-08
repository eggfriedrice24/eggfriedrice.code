//! `efr history`: the recent conversations, or the events of one as a transcript.
//!
//! A conversation is named by its id or by the start of it, which is matched against
//! the listed conversations.
//!
//! Every event stays in the log after a compaction of the model's context, so the
//! transcript shows the whole conversation and marks the place of each compaction with
//! the line that the turn showed, such as `context compacted (auto): 231k -> 24k
//! tokens, kept 3 turns, summary 3.2k`. `--verbose` adds what the user asked the
//! summary to keep and the summary itself.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use efr_client::Client;
use efr_protocol::{
    CallId, ConversationHistory, ConversationHistoryResult, ConversationId, ConversationsList,
    ConversationsListResult, Event, EventEnvelope, ExitRecord, Launch, Method, Origin, PageCursor,
    Seq, TurnId,
};
use efr_render::{RenderOptions, render, render_trace};

use crate::cli::HistoryArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Block, Spacing, Tone, context, sandbox};
use crate::live::effective_width;
use crate::output::Output;

/// Conversations asked for per page while matching the start of an id.
const PAGE: u32 = 100;
/// Pages read at most while matching.
const MAX_PAGES: usize = 10;
/// The shortest start of an id that is matched; shorter would match nearly all.
const MIN_PREFIX: usize = 4;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &HistoryArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let cursor = args.cursor.as_deref().map(PageCursor::new);
    let (conversation_id, picked) = match (&args.conversation, args.verbose) {
        (Some(query), _) => (resolve(&client, query).await?, None),
        // NOTE: the records of the exits are what `--verbose` is for, and the list has
        // none, so it shows the conversation that the user most likely means.
        (None, true) => match pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (id, Some(picked)),
            None => return out.out("no conversations yet\n"),
        },
        (None, false) => {
            let method = Method::ConversationsList(ConversationsList { cursor, limit: args.limit });
            let list: ConversationsListResult = client.call(method).await?;
            return out.out(&format::conversations(&list, ctx.clock.now()));
        }
    };
    let method = Method::ConversationHistory(ConversationHistory {
        conversation_id,
        cursor,
        limit: args.limit,
    });
    let page: ConversationHistoryResult = client.call(method).await?;
    let size = ctx.screen.size();
    let options = ctx.render_options(effective_width(size));
    let shown = Shown { options: &options, verbose: args.verbose, home: ctx.home.as_deref() };
    let mut text = String::new();
    if let Some(picked) = picked {
        let line = format!("{}; efr history lists them all", picked.words());
        let _ = writeln!(text, "{}", format::paint(&line, Tone::Dim, &options));
    }
    text.push_str(&transcript(conversation_id, &page, &shown));
    out.out(&text)
}

/// Why `efr history --verbose` without a conversation shows the one it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Picked {
    /// The newest conversation of this terminal.
    Terminal,
    /// The newest conversation of all, because this terminal has none.
    Newest,
}

impl Picked {
    pub(crate) fn words(self) -> &'static str {
        match self {
            Picked::Terminal => "the newest conversation of this terminal",
            Picked::Newest => "the newest conversation",
        }
    }
}

/// The conversation that `efr history --verbose` shows without one: the newest of the
/// terminal `tty`, else the newest of all; `None` when there is none.
pub(crate) async fn pick(
    client: &Client,
    tty: Option<&str>,
) -> Result<Option<(ConversationId, Picked)>, CliError> {
    if let Some(tty) = tty {
        match super::send::active_conversation(client, tty).await {
            Ok(id) => return Ok(Some((id, Picked::Terminal))),
            Err(CliError::NoActiveConversation { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let method = Method::ConversationsList(ConversationsList { cursor: None, limit: Some(1) });
    let list: ConversationsListResult = client.call(method).await?;
    Ok(list.conversations.first().map(|summary| (summary.id, Picked::Newest)))
}

/// How a transcript is shown.
pub(crate) struct Shown<'a> {
    pub(crate) options: &'a RenderOptions,
    /// Also show the record of each exit and how it was judged.
    pub(crate) verbose: bool,
    /// The home directory, which paths of the sandbox's lines start with as `~`.
    pub(crate) home: Option<&'a Path>,
}

/// The conversation that `query` names: a whole id, or the start of exactly one
/// listed conversation's id.
pub(crate) async fn resolve(client: &Client, query: &str) -> Result<ConversationId, CliError> {
    if let Ok(id) = query.parse::<ConversationId>() {
        return Ok(id);
    }
    let prefix = query.to_ascii_lowercase();
    if prefix.len() < MIN_PREFIX {
        return Err(CliError::ConversationNotFound { query: query.to_owned() });
    }
    let mut matches = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let method = Method::ConversationsList(ConversationsList {
            cursor: cursor.take(),
            limit: Some(PAGE),
        });
        let page: ConversationsListResult = client.call(method).await?;
        matches.extend(
            page.conversations
                .iter()
                .map(|summary| summary.id)
                .filter(|id| id.to_string().starts_with(&prefix)),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(CliError::ConversationNotFound { query: query.to_owned() }),
        _ => {
            Err(CliError::AmbiguousConversation { query: query.to_owned(), matches: matches.len() })
        }
    }
}

/// One page of a conversation's events as a transcript: prompts, rendered replies,
/// and dim notes for each turn's mode, model and effort, tool calls, approvals, the
/// sandbox's notes, the compactions of the context and how turns ended. Verbose, also
/// each exit's record and verdict, and the summary of each compaction.
pub(crate) fn transcript(
    conversation_id: ConversationId,
    page: &ConversationHistoryResult,
    shown: &Shown<'_>,
) -> String {
    let options = shown.options;
    let mut out = String::new();
    let header = format!("conversation {conversation_id}");
    let _ = writeln!(out, "{}", format::paint(&header, Tone::Dim, options));
    if let Some(cursor) = &page.next_cursor {
        let earlier = format!(
            "earlier events: efr history {conversation_id} --cursor {}",
            format::one_line(cursor.as_str())
        );
        let _ = writeln!(out, "{}", format::paint(&earlier, Tone::Dim, options));
    }
    let mut transcript = Transcript::new(shown, out);
    for group in in_turn_order(&page.events) {
        for envelope in group {
            transcript.event(envelope);
        }
        transcript.flush();
    }
    transcript.finish()
}

/// The events of one turn, or one event of no turn, and where they go in the
/// transcript.
struct Group<'a> {
    events: Vec<&'a EventEnvelope>,
    /// When the prompt of the turn was queued; `None` before the page.
    queued: Option<Seq>,
    /// When the turn started; `None` when it did not start on the page.
    started: Option<Seq>,
}

impl Group<'_> {
    /// The turn started, on the page or before it: its prompt is not on the page
    /// either.
    fn ran(&self) -> bool {
        self.started.is_some() || self.queued.is_none()
    }
}

/// The events of `page` by turn, each turn's events together and in their order. The
/// turns come in the order they started. A prompt that never started on the page
/// (taken back, cancelled, or still waiting) comes where it was in the queue: after
/// the turns whose prompts were queued before it. An event of no turn is a group of
/// its own at its place.
///
/// NOTE: the log is in event order, so the events of a turn and the prompts queued
/// while it ran interleave. A transcript in that order shows a queued prompt in the
/// middle of the turn before it.
fn in_turn_order(page: &[EventEnvelope]) -> Vec<Vec<&EventEnvelope>> {
    let mut groups: Vec<Group<'_>> = Vec::new();
    let mut by_turn: HashMap<TurnId, usize> = HashMap::new();
    for envelope in page {
        let at = match envelope.event.turn_id() {
            Some(turn) => *by_turn.entry(turn).or_insert_with(|| {
                groups.push(Group { events: Vec::new(), queued: None, started: None });
                groups.len() - 1
            }),
            None => {
                let seq = Some(envelope.seq);
                groups.push(Group { events: Vec::new(), queued: seq, started: seq });
                groups.len() - 1
            }
        };
        let group = &mut groups[at];
        match envelope.event {
            Event::PromptQueued { .. } if group.queued.is_none() => {
                group.queued = Some(envelope.seq);
            }
            Event::TurnStarted { .. } if group.started.is_none() => {
                group.started = Some(envelope.seq);
            }
            _ => {}
        }
        group.events.push(envelope);
    }
    // The turns that started, by the start. A turn that started before the page
    // comes first. The sort is stable, so equal keys keep the order of the page.
    let (mut ordered, mut waiting): (Vec<Group<'_>>, Vec<Group<'_>>) =
        groups.into_iter().partition(Group::ran);
    ordered.sort_by_key(|group| group.started);
    waiting.sort_by_key(|group| group.queued);
    for group in waiting {
        let at =
            ordered.iter().rposition(|other| other.queued < group.queued).map_or(0, |at| at + 1);
        ordered.insert(at, group);
    }
    ordered.into_iter().map(|group| group.events).collect()
}

/// Builds a transcript event by event.
struct Transcript<'a> {
    options: &'a RenderOptions,
    verbose: bool,
    home: Option<&'a Path>,
    out: String,
    spacing: Spacing,
    tools: HashMap<CallId, String>,
    /// Calls that ran in the sandbox, whose failure says so.
    contained: HashSet<CallId>,
    /// The record of each call's exit, for the line of its approval.
    exits: HashMap<CallId, ExitRecord>,
    /// The command of each shell call, whose approval shows each of its lines.
    commands: HashMap<CallId, String>,
    /// The newest text of a message that has not completed, which a turn that ended
    /// early never completes.
    partial: Option<(TurnId, u32, String)>,
}

impl<'a> Transcript<'a> {
    fn new(shown: &Shown<'a>, out: String) -> Transcript<'a> {
        Transcript {
            options: shown.options,
            verbose: shown.verbose,
            home: shown.home,
            out,
            spacing: Spacing::default(),
            tools: HashMap::new(),
            contained: HashSet::new(),
            exits: HashMap::new(),
            commands: HashMap::new(),
            partial: None,
        }
    }

    fn event(&mut self, envelope: &EventEnvelope) {
        match &envelope.event {
            Event::AssistantMessageUpdated { turn_id, index, offset, delta } => {
                if self.partial.as_ref().is_some_and(|(t, i, _)| (t, i) != (turn_id, index)) {
                    self.flush();
                }
                match &mut self.partial {
                    Some((_, _, text)) if *offset == text.len() as u64 => text.push_str(delta),
                    // The page may start in the middle of a message, whose start it lacks.
                    _ if *offset == 0 => self.partial = Some((*turn_id, *index, delta.clone())),
                    _ => {}
                }
            }
            Event::AssistantMessageCompleted { turn_id, index, text } => {
                if self.partial.as_ref().is_some_and(|(t, i, _)| (t, i) == (turn_id, index)) {
                    self.partial = None;
                }
                self.flush();
                self.message(text);
            }
            other => {
                self.flush();
                self.other(other);
            }
        }
    }

    fn other(&mut self, event: &Event) {
        match event {
            Event::PromptQueued { text, .. } => self.prompt(text),
            // NOTE: a turn recorded before turn settings names none.
            Event::TurnStarted { settings: Some(settings), .. } => {
                if let Some(line) = format::turn_settings(settings, true) {
                    self.note(&line);
                }
                if let Some(fallback) = &settings.fallback {
                    self.note(&sandbox::fallback_note(fallback, settings.mode));
                }
            }
            Event::ToolCallStarted { call_id, tool, input, launch, .. } => {
                self.tools.insert(*call_id, tool.clone());
                if let Some(command) = format::command_of(input) {
                    self.commands.insert(*call_id, command.to_owned());
                }
                if matches!(launch, Some(Launch::Contained { .. })) {
                    self.contained.insert(*call_id);
                }
                self.note(&format::tool_call(tool, input, format::columns(self.options)));
            }
            Event::ToolCallCompleted {
                call_id,
                is_error,
                exit_code,
                sandbox: summary,
                refusal,
                ..
            } => {
                let tool = self.tools.get(call_id).map_or("the tool", String::as_str);
                let contained = self.contained.contains(call_id)
                    || summary.as_ref().is_some_and(|summary| summary.confined);
                let setup = summary.as_ref().and_then(|summary| summary.setup_error.as_deref());
                if let Some(reason) = refusal {
                    self.note(&format::refused(tool, reason));
                } else if let Some(line) = sandbox::setup_failed(setup) {
                    self.note(&line);
                } else if let Some(mut line) = format::tool_result(tool, *is_error, *exit_code) {
                    if contained {
                        line.push_str(" (sandbox)");
                    }
                    self.note(&line);
                }
                if let Some(summary) = summary {
                    for blocked in &summary.blocked {
                        self.note(&sandbox::blocked(blocked));
                    }
                    if let Some(line) = sandbox::background_stopped(&summary.background_stopped) {
                        self.note(&line);
                    }
                    if let Some(line) = sandbox::survivors(&summary.survivors) {
                        self.note(&line);
                    }
                }
            }
            Event::ApprovalRequested { call_id, summary, exit: Some(exit), .. } => {
                let record = self.exits.get(call_id);
                let heading = sandbox::exit_heading(record)
                    .unwrap_or_else(|| vec![format::approval_summary(summary).0]);
                let lines = sandbox::exit_summary(exit, record, self.home);
                self.approval(heading, &lines, true);
            }
            Event::ApprovalRequested { call_id, summary, .. } => {
                let tool = self.tools.get(call_id).map(String::as_str);
                let command = self.commands.get(call_id).map(String::as_str);
                let (heading, asking) = format::approval_heading(summary, tool.zip(command));
                self.approval(heading, asking.as_deref().unwrap_or_default(), false);
            }
            Event::ExitRequested { call_id, kinds, grants, source, record, .. } => {
                if self.verbose {
                    let lines = sandbox::exit_record(kinds, grants, *source, record, self.home);
                    self.dim_lines(&lines);
                }
                self.exits.insert(*call_id, (**record).clone());
            }
            Event::ExitJudged { judge, verdict, category, .. } if self.verbose => {
                self.note(&sandbox::exit_judged(*judge, *verdict, category.as_deref()));
            }
            Event::SandboxSurfaceChanged { changes, .. } => {
                if let Some(line) = sandbox::surface_changed(changes, self.home) {
                    self.note(&line);
                }
            }
            Event::SurfaceQuestionRequested { changes, .. } => {
                let changes: Vec<String> = changes
                    .iter()
                    .map(|change| sandbox::surface_change(change, self.home).trim().to_owned())
                    .collect();
                self.note(&format!("{}: {}", sandbox::SURFACE_QUESTION, changes.join(", ")));
            }
            Event::SurfaceQuestionAnswered { keep, origin, .. } => {
                let origin = origin.map(format::origin);
                self.note(&sandbox::surface_answered(*keep, origin));
            }
            Event::TurnSurfaceReport { files, .. } => {
                self.dim_lines(&sandbox::surface_report(files));
            }
            Event::SandboxUnavailable { reason } => {
                let reason = format::one_line(reason);
                self.note(&format!("sandbox unavailable: {reason}; auto turns run as cautious"));
            }
            Event::ApprovalResolved { decision, origin, .. } => {
                let line =
                    format!("{} from {}", format::decision(*decision), format::origin(*origin));
                self.note(&line);
            }
            Event::ApprovalExpired { .. } => self.note("the approval expired"),
            Event::TurnSteered { text, .. } => {
                self.note(&format!("steered: {}", format::one_line(text)));
            }
            Event::ConversationCompacted(compaction) => {
                self.out.push_str(self.spacing.before(Block::Note));
                self.out.push_str(&context::compacted_rows(compaction, self.options));
                if self.verbose {
                    self.summary(compaction.focus.as_deref(), compaction.summary.as_deref());
                }
            }
            Event::TurnInterrupted { .. } => self.note("interrupted"),
            Event::TurnFailed { error, .. } => {
                self.note(&format!("failed: {}", format::one_line(&error.message)));
            }
            Event::TurnCancelled { .. } => self.note("cancelled when the daemon restarted"),
            Event::PromptWithdrawn { .. } => self.note("withdrawn before it ran"),
            Event::SteeringWithdrawn { steers, .. } => {
                let what = if steers.len() == 1 { "a steer" } else { "steers" };
                self.note(&format!("took back {what} that the model did not read"));
            }
            _ => {}
        }
    }

    /// An approval: `approval needed:` with the lines of its heading and, after them,
    /// `rest`. A heading of one line and the rest share a line, which a note cuts to
    /// the screen unless `whole`; the lines of a command of several lines stay whole.
    fn approval(&mut self, mut heading: Vec<String>, rest: &str, whole: bool) {
        if let [only] = heading.as_slice() {
            let line = match rest {
                "" => format!("approval needed: {only}"),
                rest => format!("approval needed: {only}; {rest}"),
            };
            if whole {
                self.dim_lines(&[line]);
            } else {
                self.note(&line);
            }
            return;
        }
        if let Some(first) = heading.first_mut() {
            *first = format!("approval needed: {first}");
        }
        if !rest.is_empty() {
            heading.push(rest.to_owned());
        }
        self.dim_lines(&heading);
    }

    /// What a compaction kept, for `--verbose`: the focus that the user asked for, then
    /// the summary as the model wrote it, each line dim and indented.
    fn summary(&mut self, focus: Option<&str>, summary: Option<&str>) {
        let mut lines = Vec::new();
        if let Some(focus) = focus {
            lines.push(format!("  focus: {}", format::one_line(focus)));
        }
        if let Some(summary) = summary {
            lines.push("  summary:".to_owned());
            lines.extend(format::lines(summary).lines().map(|line| match line {
                "" => String::new(),
                line => format!("    {line}"),
            }));
        }
        if !lines.is_empty() {
            self.dim_lines(&lines);
        }
    }

    fn prompt(&mut self, text: &str) {
        self.out.push_str(self.spacing.before(Block::Prompt));
        for line in format::lines(text).lines() {
            let _ = writeln!(
                self.out,
                "{}",
                format::paint(&format!("> {line}"), Tone::Bold, self.options)
            );
        }
    }

    fn message(&mut self, text: &str) {
        self.out.push_str(self.spacing.before(Block::Message));
        self.out.push_str(&render(text, self.options));
        if !self.options.is_terminal() && !text.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn note(&mut self, text: &str) {
        self.out.push_str(self.spacing.before(Block::Note));
        self.out.push_str(&render_trace(text, self.options));
    }

    /// Dim lines that keep their indentation and their whole width, unlike a note,
    /// which is cut to the screen: an exit's line and its facts must stay complete.
    fn dim_lines(&mut self, lines: &[String]) {
        self.out.push_str(self.spacing.before(Block::Note));
        for line in lines {
            let _ = writeln!(self.out, "{}", format::paint(line, Tone::Dim, self.options));
        }
    }

    fn flush(&mut self) {
        if let Some((_, _, text)) = self.partial.take() {
            self.message(&text);
        }
    }

    fn finish(mut self) -> String {
        self.flush();
        self.out
    }
}

#[cfg(test)]
mod tests;
