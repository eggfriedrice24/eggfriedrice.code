//! `efr diff`: what a turn changed in the files of its roots (its project and
//! `$SCRATCH`), from the daemon's snapshots (`conversation.diff`).
//!
//! Without `--turn` it shows the last turn of this terminal's conversation (the newest
//! whose `tty` is the terminal on stdin), else of the newest conversation, and a muted
//! line on stderr says which. On a terminal the diff is written to the scrollback in
//! the `diff.*` roles with the files' syntax colours, then a muted line counts the files
//! and the lines. Elsewhere stdout holds the diff alone, as the daemon sent it, so it
//! can go to `git apply` or a pager. `--stat` lists the files instead, after a table of
//! the groups of the files (`[diff.groups]`, then generated, tests, docs and code) when
//! a file is not code. `--stat --all` adds up every turn of the conversation from the
//! ends of its turns in its history (`conversation.history`), without a turn that
//! `efr undo` took back. A turn that changed nothing says so on stderr, and a turn
//! without a snapshot is an error.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use efr_config::DiffGroups;
use efr_protocol::{
    ChangeKind, ConversationDiff, ConversationDiffResult, ConversationHistory,
    ConversationHistoryResult, ConversationId, ErrorCode, Event, FileChange, FileChanges, Method,
    Origin, TurnId,
};
use efr_render::{RenderOptions, render};
use efr_transport::client::{Client, ClientError};

use super::history::Picked;
use crate::cli::DiffArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, changes};
use crate::live::effective_width;
use crate::output::Output;

/// The most pages of a conversation's history that `--all` reads.
const MAX_HISTORY_PAGES: usize = 1_000;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &DiffArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let (conversation_id, picked) = match (&args.conversation, args.turn) {
        (Some(query), _) => (Some(super::history::resolve(&client, query).await?), None),
        // NOTE: a turn id names its conversation, so the daemon finds it.
        (None, Some(_)) => (None, None),
        (None, None) => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => (Some(id), Some(picked)),
            None => return Err(CliError::NoConversation),
        },
    };
    let groups = &ctx.settings.diff.groups;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    if args.all
        && let Some(conversation_id) = conversation_id
    {
        return all_turns(&client, out, conversation_id, picked, groups, &options).await;
    }
    let method = Method::ConversationDiff(ConversationDiff {
        conversation_id,
        turn_id: args.turn,
        stat: args.stat,
    });
    let result: ConversationDiffResult = match client.call(method).await {
        Ok(result) => result,
        Err(ClientError::Server { body }) if body.code == ErrorCode::NotFound => {
            let what = match (args.turn, conversation_id) {
                (Some(turn), _) => format!("turn {turn}"),
                (None, Some(id)) => format!("the last turn of conversation {id}"),
                (None, None) => "the last turn".to_owned(),
            };
            return Err(CliError::NoDiff { what, reason: body.message });
        }
        Err(error) => return Err(error.into()),
    };
    if let Some(picked) = picked {
        let line =
            format!("the last turn of {}; efr diff --turn <id> shows another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
    if result.changes.count() == 0 {
        out.err(&format!("turn {} changed no files\n", result.turn_id));
        return Ok(());
    }
    out.out(&shown(&result, args.stat, groups, &options))
}

/// `efr diff --stat --all`: the files of every turn of `conversation_id` that changed
/// files and that no undo took back, added up.
async fn all_turns(
    client: &Client,
    out: &mut Output,
    conversation_id: ConversationId,
    picked: Option<Picked>,
    groups: &DiffGroups,
    options: &RenderOptions,
) -> Result<(), CliError> {
    let mut pages = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_HISTORY_PAGES {
        let method = Method::ConversationHistory(ConversationHistory {
            conversation_id,
            cursor: cursor.take(),
            limit: None,
        });
        let page: ConversationHistoryResult = client.call(method).await?;
        cursor = page.next_cursor;
        pages.push(page.events);
        if cursor.is_none() {
            break;
        }
    }
    // NOTE: the first page is the newest; the events of each page are oldest first.
    let events = pages.iter().rev().flatten().map(|envelope| &envelope.event);
    let (turns, summed) = sum_turns(events);
    if let Some(picked) = picked {
        let unit = if turns == 1 { "turn" } else { "turns" };
        let line = format!("{turns} {unit} of {} that changed files", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, options)));
    }
    if summed.count() == 0 {
        out.err(&format!("no turn of conversation {conversation_id} changed files\n"));
        return Ok(());
    }
    out.out(&changes::stat(&summed, groups, options))
}

/// The number of turns among `events` (in the order of the log) that changed files
/// and that no undo took back, and their files added up: one entry per path with the
/// lines of every turn and the kind of [`summed_kind`].
///
/// Two limits: a path is relative to the project root of its turn, so the same path in
/// the turns of two projects is one entry. And the count of the files that the lists
/// leave out (`more`) is the sum of the turns, so a file that one turn lists and
/// another leaves out counts twice.
pub(crate) fn sum_turns<'a>(events: impl IntoIterator<Item = &'a Event>) -> (usize, FileChanges) {
    let mut ended: Vec<(TurnId, &FileChanges)> = Vec::new();
    let mut undone: HashSet<TurnId> = HashSet::new();
    for event in events {
        match event {
            Event::TurnCompleted { turn_id, changes: Some(changes), .. }
            | Event::TurnInterrupted { turn_id, changes: Some(changes), .. }
            | Event::TurnFailed { turn_id, changes: Some(changes), .. }
                if changes.count() > 0 =>
            {
                ended.push((*turn_id, changes));
            }
            Event::UndoApplied { undone_turn_id, redo: false, .. } => {
                undone.insert(*undone_turn_id);
            }
            Event::UndoApplied { undone_turn_id, redo: true, .. } => {
                undone.remove(undone_turn_id);
            }
            _ => {}
        }
    }
    let kept: Vec<&FileChanges> = ended
        .iter()
        .filter(|(turn, _)| !undone.contains(turn))
        .map(|(_, changes)| *changes)
        .collect();
    // NOTE: each path keeps the kind that its first turn gave it, next to the entry.
    let mut files: BTreeMap<&str, (ChangeKind, FileChange)> = BTreeMap::new();
    let mut summed = FileChanges::default();
    for changes in &kept {
        summed.more = summed.more.saturating_add(changes.more);
        summed.added = summed.added.saturating_add(changes.added);
        summed.removed = summed.removed.saturating_add(changes.removed);
        for file in &changes.files {
            match files.get_mut(file.path.as_str()) {
                Some((first, known)) => {
                    let kind = summed_kind(*first, known.kind, file.kind);
                    if kind != ChangeKind::Renamed {
                        known.from = None;
                    }
                    known.kind = kind;
                    known.added = known.added.saturating_add(file.added);
                    known.removed = known.removed.saturating_add(file.removed);
                    known.binary |= file.binary;
                }
                None => {
                    files.insert(&file.path, (file.kind, file.clone()));
                }
            }
        }
    }
    summed.files = files.into_values().map(|(_, file)| file).collect();
    (kept.len(), summed)
}

/// The kind of a path over several turns: `first` from its first turn, `so_far` from
/// the turns up to the last one, `last` from the last one. The kind stays when the
/// turns agree. Else it follows whether the file was there before the first turn and
/// after the last: a file that the first turn made is new, or deleted when a later turn
/// deleted it; a file that a turn deleted is deleted; a renamed file that later turns
/// changed stays renamed; anything else is modified.
fn summed_kind(first: ChangeKind, so_far: ChangeKind, last: ChangeKind) -> ChangeKind {
    match (first, last) {
        _ if so_far == last => last,
        (_, ChangeKind::Deleted) => ChangeKind::Deleted,
        (ChangeKind::Added, _) => ChangeKind::Added,
        (ChangeKind::Renamed, ChangeKind::Modified) if so_far == ChangeKind::Renamed => {
            ChangeKind::Renamed
        }
        _ => ChangeKind::Modified,
    }
}

/// What `efr diff` writes to stdout for `result`: the list of files with `stat` or when
/// the daemon sent no diff, else the diff. On a terminal the diff is painted and a
/// muted line counts the files and the lines; elsewhere it is the daemon's text, with
/// control characters other than tabs and newlines as visible stand-ins.
pub(crate) fn shown(
    result: &ConversationDiffResult,
    stat: bool,
    groups: &DiffGroups,
    options: &RenderOptions,
) -> String {
    let diff = match result.diff.as_deref() {
        Some(diff) if !stat => diff,
        _ => return changes::stat(&result.changes, groups, options),
    };
    if !options.is_terminal() {
        let mut text = format::diff_text(diff).into_owned();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        return text;
    }
    let (body, cut) = changes::split_cut(diff);
    let mut out = render(&format::code_block("diff", body), options);
    if cut > 0 {
        let _ = writeln!(out, "{}", format::paint(&changes::more_lines(cut), Tone::Dim, options));
    }
    if let Some(line) = changes::turn_line(&result.changes, groups) {
        let _ = write!(out, "\n{}\n", format::paint(&line, Tone::Dim, options));
    }
    out
}

#[cfg(test)]
mod tests;
