//! `edit`: replaces exact text in one file, in the shape of Claude Code's edit tool.
//!
//! The engine (`efr_patch::replace`) finds `old_string` in the text that this tool read
//! and computes the new content: one exact, unique match, or every match with
//! `replace_all`. This tool does the IO around it with the checks of `apply_patch`: it
//! refuses a path through a link, anything that is not a regular file, a file over
//! 16 MiB and a binary file. It records the original in the write journal, then writes
//! atomically. It changes one file that exists, and never makes, deletes or moves one.

use std::borrow::Cow;
use std::path::PathBuf;

use async_trait::async_trait;
use efr_patch::{Occurrences, PatchError, Replacement};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::apply_patch::{near_line, original};
use crate::diff::written_diff;
use crate::paths::resolve;
use crate::tool::parse_input;
use crate::write_file::{blocking, write};
use crate::{
    JournalEntry, Original, Tool, ToolContext, ToolError, ToolOutputSink, ToolRequirements,
    ToolResult, ToolSpec, WrittenFile, WrittenKind,
};

/// What the model reads about the tool. It promises no check that the model read the
/// file first, because the tool makes none.
const DESCRIPTION: &str = "\
Replace exact text in one file. Use this tool for each change to a file: do not write a \
whole file again with write_file for a small change, and do not use sed -i or perl -pi. \
`old_string` is the exact text of the file that changes, with its indentation and line \
breaks, copied from what you read. It must occur exactly once: give enough lines around \
the change to make it unique. Set `replace_all` to true to replace every occurrence, such \
as to rename a variable. `new_string` replaces it and must differ from it. The file must \
exist; to make a new file, use write_file. Paths are relative to the user's working \
directory, start with ~ for the home directory, or are absolute.";

/// What the model reads for an empty `old_string`.
const EMPTY_OLD: &str = "old_string is empty. Use write_file to make a new file.";

/// What the model reads when `old_string` and `new_string` are the same.
const NO_CHANGE: &str = "No change: old_string and new_string are the same.";

/// The input of `edit`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    /// The file to change: absolute, relative to the user's working directory, or under
    /// `~`.
    path: String,
    /// The exact text to replace.
    old_string: String,
    /// The text that replaces it. It must differ from `old_string`.
    new_string: String,
    /// Replace every occurrence of `old_string`. By default it must occur exactly once.
    #[serde(default)]
    replace_all: bool,
}

/// Replaces exact text in one file: `old_string` becomes `new_string`, once or at every
/// place (`replace_all`).
///
/// It declares its path for writing, and is never destructive. Before it writes the
/// file it records the original in the context's journal, and it writes nothing when the
/// journal fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct EditTool;

impl EditTool {
    /// The tool's name.
    pub const NAME: &'static str = "edit";

    /// The call's input and its absolute path, or why it is not one.
    fn plan(&self, ctx: &ToolContext, input: &Value) -> Result<(EditInput, PathBuf), ToolError> {
        let input: EditInput = parse_input(Self::NAME, input)?;
        let path = resolve(ctx, &input.path)?;
        Ok((input, path))
    }
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::for_input::<EditInput>(Self::NAME, DESCRIPTION)
    }

    fn requirements(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        let (_, path) = self.plan(ctx, input)?;
        Ok(ToolRequirements::none().with_write(path))
    }

    /// The diff of the change, in the form of a file of `apply_patch`'s preview, within
    /// the bounds of a written diff. Nothing for a call that fails when it runs.
    async fn preview(&self, ctx: &ToolContext, input: &Value) -> Option<String> {
        let (input, path) = self.plan(ctx, input).ok()?;
        if refusal(&input).is_some() {
            return None;
        }
        let home = ctx.home.clone();
        let target = path.clone();
        let (_, text) = blocking(&path, move || original(&home, &target)).await.ok()?;
        let text = text?;
        let replaced = replaced(&text, &input).ok()?;
        let max_lines = efr_protocol::MAX_CALL_DIFF_LINES;
        written_diff(&path, Some(&text), &replaced.text, max_lines).map(|diff| diff.text)
    }

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        _out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let (input, path) = self.plan(&ctx, &input)?;
        if let Some(refusal) = refusal(&input) {
            return Ok(ToolResult::error(refusal));
        }
        let home = ctx.home.clone();
        let target = path.clone();
        let (snapshot, text) = blocking(&path, move || original(&home, &target)).await?;
        let Some(text) = text else {
            return Ok(ToolResult::error(format!(
                "{} does not exist. Use write_file to make a new file.",
                path.display()
            )));
        };
        let replaced = match replaced(&text, &input) {
            Ok(replaced) => replaced,
            Err(failure) => return Ok(ToolResult::error(failure)),
        };
        let (mode, owner) = match &snapshot.original {
            Original::File { mode, uid, gid, .. } => (*mode, Some((*uid, *gid))),
            // NOTE: the text came from a file, so its snapshot is a file too.
            _ => unreachable!("a file with a text has a file snapshot"),
        };
        let diff =
            written_diff(&path, Some(&text), &replaced.text, efr_protocol::MAX_CALL_DIFF_LINES);
        let written =
            WrittenFile { path: path.clone(), kind: WrittenKind::Changed, binary: false, diff };
        // NOTE: the original goes to the journal before the write. When the user
        // interrupts the turn while the journal records it, the call's future is dropped
        // and nothing is written.
        ctx.journal.record(JournalEntry::new(ctx.ids, snapshot)).await?;
        let target = path.clone();
        let content = replaced.text;
        blocking(&path, move || write(&target, content.as_bytes(), mode, owner)).await?;
        let matches = match replaced.count {
            1 => "1 match".to_owned(),
            count => format!("{count} matches"),
        };
        Ok(ToolResult::ok(format!("Success. Replaced {matches} in {}.", input.path))
            .with_written(vec![written]))
    }
}

/// What the model reads for an input that can change nothing, whatever the file holds:
/// an empty `old_string`, or the same text on both sides.
fn refusal(input: &EditInput) -> Option<&'static str> {
    if input.old_string.is_empty() {
        Some(EMPTY_OLD)
    } else if input.old_string == input.new_string {
        Some(NO_CHANGE)
    } else {
        None
    }
}

/// The new text of the file `text` after the call's replacement, or what the model
/// reads when `old_string` does not occur as the call asks.
fn replaced(text: &str, input: &EditInput) -> Result<Replacement, String> {
    let (old, new) = with_line_ends(text, &input.old_string, &input.new_string);
    let occurrences = if input.replace_all { Occurrences::All } else { Occurrences::One };
    efr_patch::replace(text, &old, &new, occurrences).map_err(|error| failure(&error))
}

/// `old` and `new` with the line ends of `text`. In a file whose every line ends with
/// `\r\n`, a model that wrote both strings with `\n` alone means `\r\n`: the exact
/// match would fail on every line break, and a new line would get the wrong end. A
/// string that holds a `\r` is taken as written.
fn with_line_ends<'a>(text: &str, old: &'a str, new: &'a str) -> (Cow<'a, str>, Cow<'a, str>) {
    let crlf = text.matches("\r\n").count();
    let only_crlf = crlf > 0 && crlf == text.matches('\n').count();
    if !only_crlf || old.contains('\r') || new.contains('\r') {
        return (Cow::Borrowed(old), Cow::Borrowed(new));
    }
    (Cow::Owned(old.replace('\n', "\r\n")), Cow::Owned(new.replace('\n', "\r\n")))
}

/// What the model reads when the replacement cannot be made: how often `old_string`
/// occurs, or the nearest lines of the file when it does not occur.
fn failure(error: &PatchError) -> String {
    match error {
        PatchError::NotUnique { count } => format!(
            "Found {count} matches of old_string, but replace_all is false. Give more context \
             to make one match, or set replace_all to true."
        ),
        PatchError::NotFound { nearest } => {
            let mut lines = vec!["old_string was not found in the file.".to_owned()];
            if nearest.is_empty() {
                lines.push("No line of the file is near it.".to_owned());
            } else {
                lines.push("The nearest lines are:".to_owned());
                lines.extend(nearest.iter().map(near_line));
            }
            lines.push("Read the file again and copy old_string exactly as it is.".to_owned());
            lines.join("\n")
        }
        PatchError::EmptyOld => EMPTY_OLD.to_owned(),
        other => format!("{other}."),
    }
}

#[cfg(test)]
mod tests;
