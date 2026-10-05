//! `efr history`: the recent conversations, or the events of one as a transcript.
//!
//! A conversation is named by its id or by the start of it, which is matched against
//! the listed conversations.

use std::collections::HashMap;
use std::fmt::Write as _;

use efr_client::Client;
use efr_protocol::{
    CallId, ConversationHistory, ConversationHistoryResult, ConversationId, ConversationsList,
    ConversationsListResult, Event, EventEnvelope, Method, Origin, PageCursor, TurnId,
};
use efr_render::{RenderOptions, render, render_trace};

use crate::cli::HistoryArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Block, Spacing, Tone};
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
    let Some(query) = &args.conversation else {
        let method = Method::ConversationsList(ConversationsList { cursor, limit: args.limit });
        let list: ConversationsListResult = client.call(method).await?;
        return out.out(&format::conversations(&list, ctx.clock.now()));
    };
    let conversation_id = resolve(&client, query).await?;
    let method = Method::ConversationHistory(ConversationHistory {
        conversation_id,
        cursor,
        limit: args.limit,
    });
    let page: ConversationHistoryResult = client.call(method).await?;
    let size = ctx.screen.size();
    let options = ctx.term.render_options(effective_width(size), ctx.settings.theme);
    out.out(&transcript(conversation_id, &page, &options))
}

/// The conversation that `query` names: a whole id, or the start of exactly one
/// listed conversation's id.
async fn resolve(client: &Client, query: &str) -> Result<ConversationId, CliError> {
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
/// and dim notes for each turn's mode, model and effort, tool calls, approvals and how
/// turns ended.
pub(crate) fn transcript(
    conversation_id: ConversationId,
    page: &ConversationHistoryResult,
    options: &RenderOptions,
) -> String {
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
    let mut transcript = Transcript::new(options, out);
    for envelope in &page.events {
        transcript.event(envelope);
    }
    transcript.finish()
}

/// Builds a transcript event by event.
struct Transcript<'a> {
    options: &'a RenderOptions,
    out: String,
    spacing: Spacing,
    tools: HashMap<CallId, String>,
    /// The newest text of a message that has not completed, which a turn that ended
    /// early never completes.
    partial: Option<(TurnId, u32, String)>,
}

impl<'a> Transcript<'a> {
    fn new(options: &'a RenderOptions, out: String) -> Transcript<'a> {
        let spacing = Spacing::default();
        Transcript { options, out, spacing, tools: HashMap::new(), partial: None }
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
            }
            Event::ToolCallStarted { call_id, tool, input, .. } => {
                self.tools.insert(*call_id, tool.clone());
                self.note(&format::tool_call(tool, input));
            }
            Event::ToolCallCompleted { call_id, is_error, exit_code, .. } => {
                let tool = self.tools.get(call_id).map_or("the tool", String::as_str);
                if let Some(line) = format::tool_result(tool, *is_error, *exit_code) {
                    self.note(&line);
                }
            }
            Event::ApprovalRequested { summary, .. } => {
                self.note(&format!("approval needed: {}", format::one_line(summary)));
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
            Event::TurnInterrupted { .. } => self.note("interrupted"),
            Event::TurnFailed { error, .. } => {
                self.note(&format!("failed: {}", format::one_line(&error.message)));
            }
            Event::TurnCancelled { .. } => self.note("cancelled when the daemon restarted"),
            _ => {}
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
