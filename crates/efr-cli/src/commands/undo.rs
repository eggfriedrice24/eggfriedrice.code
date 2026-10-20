//! `efr undo`: take back the file and ref changes of a turn, from efr's snapshots
//! (`turn.undo`).
//!
//! Without `--turn` it takes the newest turn of this terminal's conversation that is
//! not undone, else of the newest conversation, and a muted line on stderr says which.
//! It asks the daemon twice: first for a preview (a dry run), which it shows one row
//! per path (`restore`, `trash`, `delete`, `ref`, `skip`, `not covered`, `command`),
//! then, after the question `undo? y = yes, n = no` or with `--yes`, for the undo
//! itself. The daemon checks each path again when it applies. Without a terminal and
//! without `--yes` it shows the preview, says so, and changes nothing. `--dry-run`
//! stops after the preview, `--list` lists the turns that can be undone, `--redo` takes
//! back the newest undo, and `--force` also overwrites a path that changed after the
//! turn. When only `--force` would act on any row, `efr undo` without it changes
//! nothing and fails with that hint, before the question. A refusal (a turn runs, nothing to undo) is an error with the daemon's reason.

use std::fmt::Write as _;

use efr_protocol::{
    ErrorBody, ErrorCode, Method, Origin, RefChange, TurnUndo, TurnUndoResult, UndoAction,
    UndoEntry, UndoReason, UndoableTurn,
};
use efr_render::RenderOptions;
use efr_transport::client::{Client, ClientError};

use crate::cli::UndoArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone};
use crate::keys::{self, Key};
use crate::live::effective_width;
use crate::output::Output;

/// The question before an undo.
const QUESTION: &str = "undo? y = yes, n = no";

/// The question before a redo.
const REDO_QUESTION: &str = "redo? y = yes, n = no";

/// The width of the action word of a row, so the paths start in one column.
const ACTION_WIDTH: usize = 12;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &UndoArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let conversation_id = match (&args.conversation, args.turn) {
        (Some(query), _) => Some(super::history::resolve(&client, query).await?),
        // NOTE: a turn id names its conversation, so the daemon finds it.
        (None, Some(_)) => None,
        (None, None) => match super::history::pick(&client, ctx.tty.as_deref()).await? {
            Some((id, picked)) => {
                let line =
                    format!("{}; efr undo --conversation <id> undoes in another", picked.words());
                out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
                Some(id)
            }
            None => return Err(CliError::NothingToUndo),
        },
    };
    let params = |ctx: &Context, dry_run: bool| TurnUndo {
        command_id: ctx.command_id(),
        conversation_id,
        turn_id: args.turn,
        dry_run,
        force: args.force,
        redo: args.redo,
        list: false,
    };
    if args.list {
        let list = TurnUndo { list: true, ..params(ctx, false) };
        let result = call(&client, list).await?;
        return out.out(&turns(&result.turns, &options));
    }
    let preview = call(&client, params(ctx, true)).await?;
    out.out(&preview_rows(&preview, args.redo, &options))?;
    if preview.entries.is_empty() && preview.refs.is_empty() {
        out.err(&format!("{}\n", format::paint("nothing to undo", Tone::Dim, &options)));
        return Ok(());
    }
    if args.dry_run {
        return Ok(());
    }
    let count = needs_force(&preview);
    if !args.force && !args.redo && count > 0 {
        return Err(CliError::UndoNeedsForce { count });
    }
    if !args.yes {
        if !ctx.keys.available() {
            return Err(CliError::UndoNeedsYes);
        }
        let question = if args.redo { REDO_QUESTION } else { QUESTION };
        out.out(&format!("{}\n", format::paint(question, Tone::Attention, &options)))?;
        if !answer(ctx).await? {
            out.err(&format!("{}\n", format::paint("nothing changed", Tone::Dim, &options)));
            return Ok(());
        }
    }
    // NOTE: the turn of the preview, so a turn that ended meanwhile is not the one that
    // the apply takes back.
    let turn_id = if args.redo { args.turn } else { preview.turn_id.or(args.turn) };
    let apply = TurnUndo { turn_id, ..params(ctx, false) };
    let mut result = call(&client, apply).await?;
    // NOTE: the preview showed its notes already; the report adds only new ones, such
    // as a write that failed.
    result.notes.retain(|note| !preview.notes.contains(note));
    out.out(&report(&result, args.redo, ctx.home.as_deref(), &options))
}

/// The rows of `preview` that only `--force` acts on, or 0 when another row acts
/// without it: then the undo writes, and the rows stay as they are.
fn needs_force(preview: &TurnUndoResult) -> usize {
    let forced = |reason: Option<UndoReason>| {
        matches!(
            reason,
            Some(
                UndoReason::ChangedAfterTurn
                    | UndoReason::NoRecordAfterTurn
                    | UndoReason::LaterTurnNotUndone
            )
        )
    };
    let acts = preview
        .entries
        .iter()
        .any(|entry| !matches!(entry.action, UndoAction::Skip | UndoAction::NotCovered))
        || preview.refs.iter().any(|change| change.reason.is_none());
    if acts {
        return 0;
    }
    preview.entries.iter().filter(|entry| forced(entry.reason)).count()
        + preview.refs.iter().filter(|change| forced(change.reason)).count()
}

/// Calls `turn.undo` with `params`; a refusal becomes the error that names its reason.
async fn call(client: &Client, params: TurnUndo) -> Result<TurnUndoResult, CliError> {
    match client.call(Method::TurnUndo(params)).await {
        Ok(result) => Ok(result),
        Err(ClientError::Server { body })
            if matches!(
                body.code,
                ErrorCode::Conflict | ErrorCode::Busy | ErrorCode::Forbidden
            ) =>
        {
            Err(CliError::UndoRefused { body })
        }
        Err(error) => Err(error.into()),
    }
}

/// Reads keys until `y` or `n`; Esc says no, and Ctrl+C stops `efr`.
pub(crate) async fn answer(ctx: &Context) -> Result<bool, CliError> {
    let mut reader = ctx.keys.start()?;
    let mut interrupt = ctx.interrupt.wait();
    let answer = loop {
        tokio::select! {
            key = reader.next() => match key {
                Some(Key::Esc) | None => break Ok(false),
                Some(Key::Byte(byte)) => {
                    if let Some(decision) = keys::decision(byte) {
                        break Ok(decision == efr_protocol::ApprovalDecision::Allow);
                    }
                }
            },
            () = &mut interrupt => break Err(CliError::Interrupted),
        }
    };
    reader.stop().await;
    answer
}

/// The preview of `result`: what the undo (or the redo) of its turn does, one row per
/// path and ref, then its notes.
pub(crate) fn preview_rows(result: &TurnUndoResult, redo: bool, options: &RenderOptions) -> String {
    let mut out = String::new();
    if let Some(turn_id) = result.turn_id {
        let verb = if redo { "redo the undo of turn" } else { "undo turn" };
        let mut head = format!("{verb} {turn_id}");
        let title = result.title.as_deref().and_then(|title| title.lines().next());
        if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
            let _ = write!(head, ": {}", format::one_line(title));
        }
        if result.resumed {
            head.push_str(" (goes on with an undo that stopped half way)");
        }
        let _ = writeln!(out, "{}", format::paint(&head, Tone::Bold, options));
    }
    for entry in &result.entries {
        let _ = writeln!(out, "{}", entry_row(entry, options));
    }
    if result.entries_more > 0 {
        let more = format!("\u{2026} {} more", result.entries_more);
        let _ = writeln!(out, "{}", format::paint(&more, Tone::Dim, options));
    }
    let roots: Vec<&str> = result.refs.iter().map(|change| change.root.as_str()).collect();
    let several = roots.windows(2).any(|pair| pair[0] != pair[1]);
    for change in &result.refs {
        let _ = writeln!(out, "{}", ref_row(change, several, options));
    }
    for note in &result.notes {
        let _ = writeln!(out, "{}", format::paint(&format::one_line(note), Tone::Dim, options));
    }
    out
}

/// One row of a path: its action, its path, and why for a skip or a path that no
/// snapshot covers.
fn entry_row(entry: &UndoEntry, options: &RenderOptions) -> String {
    let (word, tone) = match entry.action {
        UndoAction::Restore => ("restore", Tone::Plain),
        UndoAction::Trash => ("trash", Tone::Plain),
        UndoAction::Delete => ("delete", Tone::Plain),
        UndoAction::Command => ("command", Tone::Plain),
        UndoAction::Skip => ("skip", Tone::Dim),
        UndoAction::NotCovered => ("not covered", Tone::Dim),
        _ => ("other", Tone::Dim),
    };
    let mut row = format!("{word:<ACTION_WIDTH$}{}", format::one_line(&entry.path));
    if let Some(reason) = entry.reason {
        let _ = write!(row, " ({})", reason_words(reason));
    }
    format::paint(&row, tone, options)
}

/// One row of a ref: `ref main 1a2b3c4 -> 5d6e7f8`, with its root when the undo moves
/// the refs of more than one root, and why when it does not move it.
fn ref_row(change: &RefChange, root: bool, options: &RenderOptions) -> String {
    let id = |id: &Option<String>| id.as_deref().map_or("none".to_owned(), format::one_line);
    let mut row = format!(
        "{:<ACTION_WIDTH$}{} {} -> {}",
        "ref",
        format::one_line(&change.name),
        id(&change.from),
        id(&change.to)
    );
    if root {
        let _ = write!(row, " in {}", format::one_line(&change.root));
    }
    let tone = match (change.applied, change.reason) {
        (Some(false), _) | (_, Some(_)) => Tone::Dim,
        _ => Tone::Plain,
    };
    if let Some(reason) = change.reason {
        let _ = write!(row, " ({})", reason_words(reason));
    } else if change.applied == Some(false) {
        row.push_str(" (not moved)");
    }
    format::paint(&row, tone, options)
}

/// Why undo leaves a path or a ref as it is, in a few words.
pub(crate) fn reason_words(reason: UndoReason) -> &'static str {
    match reason {
        UndoReason::ChangedAfterTurn => "changed after the turn; --force overwrites it",
        UndoReason::NoRecordAfterTurn => {
            "efr has no record after the turn; --force uses its last snapshot"
        }
        UndoReason::LeftOut => "the snapshot left it out",
        UndoReason::Masked => "masked",
        UndoReason::OverLimit => "over the size limit",
        UndoReason::NewInTurn => "new in the turn",
        UndoReason::LaterTurnNotUndone => "a later turn changed it; --force overwrites it",
        UndoReason::NoMirror => "efr has no copy of the refs",
        UndoReason::ObjectsMissing => "its objects are gone",
        UndoReason::OutsideTurnCalls => "changed outside the turn's calls",
        UndoReason::AlreadyDone => "already as before the turn",
        UndoReason::RootReplaced => "the directory was replaced; --force writes it anyway",
        UndoReason::Unknown => "efr does not know its state",
        UndoReason::CallUnfinished => "the call did not finish",
        UndoReason::SnapshotFailed => "a snapshot failed",
        UndoReason::StoreChanged => "the snapshot store changed",
        UndoReason::StoreFull => "the store dropped its copy",
        UndoReason::StagedOnly => "only staged",
        _ => "another reason",
    }
}

/// The lines after an applied undo: `undone: 2 files restored, 1 file moved to <trash>`,
/// or `undo stopped: ...` after a write that failed, the refs that it moved, then its
/// notes.
pub(crate) fn report(
    result: &TurnUndoResult,
    redo: bool,
    home: Option<&std::path::Path>,
    options: &RenderOptions,
) -> String {
    let count = |wanted: &[UndoAction]| {
        result.entries.iter().filter(|entry| wanted.contains(&entry.action)).count()
    };
    let files =
        |count: usize| if count == 1 { "1 file".to_owned() } else { format!("{count} files") };
    let mut parts = Vec::new();
    let restored = count(&[UndoAction::Restore, UndoAction::Command]);
    if restored > 0 {
        parts.push(format!("{} restored", files(restored)));
    }
    let trashed = count(&[UndoAction::Trash]);
    if trashed > 0 {
        let to = match &result.trash {
            Some(trash) => format!(" to {}/", format::sandbox::tilde(trash, home)),
            None => " to the undo trash".to_owned(),
        };
        parts.push(format!("{} moved{to}", files(trashed)));
    }
    let deleted = count(&[UndoAction::Delete]);
    if deleted > 0 {
        let unit = if deleted == 1 { "link" } else { "links" };
        parts.push(format!("{deleted} {unit} removed"));
    }
    let moved = result.refs.iter().filter(|change| change.applied == Some(true)).count();
    if moved > 0 {
        let unit = if moved == 1 { "ref" } else { "refs" };
        parts.push(format!("{moved} {unit} moved"));
    }
    let skipped = count(&[UndoAction::Skip, UndoAction::NotCovered]);
    if skipped > 0 {
        parts.push(format!("{skipped} left as they are"));
    }
    let verb = match (redo, result.stopped) {
        (false, false) => "undone",
        (true, false) => "redone",
        (false, true) => "undo stopped",
        (true, true) => "redo stopped",
    };
    let line = match (result.applied || result.stopped, parts.is_empty()) {
        (false, _) => format!("{verb}: nothing changed"),
        (true, true) => verb.to_owned(),
        (true, false) => format!("{verb}: {}", parts.join(", ")),
    };
    let tone = if result.stopped { Tone::Attention } else { Tone::Success };
    let mut out = format!("{}\n", format::paint(&line, tone, options));
    if result.resumed {
        let _ = writeln!(
            out,
            "{}",
            format::paint("this went on with an undo that stopped half way", Tone::Dim, options)
        );
    }
    for note in &result.notes {
        let _ = writeln!(out, "{}", format::paint(&format::one_line(note), Tone::Dim, options));
    }
    out
}

/// `efr undo --list`: one row per turn, newest first: its id, its state and its title.
pub(crate) fn turns(turns: &[UndoableTurn], options: &RenderOptions) -> String {
    if turns.is_empty() {
        return format!("{}\n", format::paint("no turn can be undone", Tone::Dim, options));
    }
    let mut out = String::new();
    for turn in turns {
        let state = match (turn.undone, turn.stopped) {
            (true, _) => "undone",
            (false, true) => "stopped",
            (false, false) => "",
        };
        let mode = match (turn.full, turn.auto) {
            (true, _) => "full",
            (false, true) => "auto",
            (false, false) => "",
        };
        let title = turn.title.as_deref().map(format::one_line).unwrap_or_default();
        let row = format!("{}  {state:<7}  {mode:<4}  {title}", turn.turn_id);
        let tone = if turn.undone { Tone::Dim } else { Tone::Plain };
        let _ = writeln!(out, "{}", format::paint(row.trim_end(), tone, options));
    }
    out
}

/// The refusal kind in the data of a refused `turn.undo`, such as `later_turns_not_undone`.
pub(crate) fn refused_kind(body: &ErrorBody) -> Option<&str> {
    body.data.as_ref()?.get("refused")?.get("kind")?.as_str()
}

#[cfg(test)]
mod tests;
