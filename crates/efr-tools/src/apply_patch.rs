//! `apply_patch`: edits files with a patch in the `apply_patch` format of Codex, all or
//! nothing.
//!
//! The engine (`efr-patch`) parses the patch and computes every new content from the
//! texts that this tool read; this tool does the IO around it. It reads every file of
//! the patch first and refuses a path through a link, anything that is not a regular
//! file, a file over 16 MiB and a binary file. Only when every hunk matches does it
//! write: for each change it records the originals in the write journal, then writes
//! atomically. When a write fails, it puts back every file that it changed before, so
//! a patch over several files changes all of them or none.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use efr_patch::{ChangeKind, FileChange, Files, Operation, Patch, PatchError};
use efr_scope::Home;
use serde::de::Error as _;
use serde_json::Value;

use crate::diff::{UNCHANGED, change_diff};
use crate::paths::{check_real, resolve};
use crate::write_file::{blocking, write};
use crate::{
    FileSnapshot, JournalEntry, Original, Tool, ToolContext, ToolError, ToolGrammar,
    ToolOutputSink, ToolRequirements, ToolResult, ToolSpec, WrittenFile, WrittenKind,
    freeform_text,
};

/// The largest file the tool reads: its original must fit in the journal.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// How much of the start of a file is searched for a NUL byte, which marks it binary.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// The mode of a file the tool creates. An existing file keeps its own.
const NEW_FILE_MODE: u32 = 0o644;

/// The most bytes of an approval's preview: each file has the bounds of a written
/// diff, and the files after this many bytes are only counted, so the event stays far
/// below the frame limit.
const MAX_PREVIEW_BYTES: usize = 4 * 1024 * 1024;

/// The last sentence of every failure after the patch was read.
const NOTHING_CHANGED: &str = "No file was changed.";

/// What the model reads about the tool.
const DESCRIPTION: &str = "\
Edit files with a patch. Use this tool for each change to a file: do not write a whole \
file again for a small change, and do not use sed -i or perl -pi. The input is the \
patch as plain text, not JSON (when the tool takes a JSON object, put the whole patch \
in `input`). The format:

*** Begin Patch
*** Update File: src/app.py
@@ def main():
     setup()
-    run(1)
+    run(2)
     report()
*** Add File: docs/notes.md
+# Notes
+First line.
*** Delete File: old/unused.py
*** Update File: src/a.py
*** Move to: src/b.py
@@
-import os
+import sys
*** End Patch

Each file operation starts with `*** Add File: <path>`, `*** Delete File: <path>` or \
`*** Update File: <path>`. An update can move the file with `*** Move to: <path>` on \
the next line. In an update, each line of a hunk starts with a space (a line that \
stays), `-` (a line to remove) or `+` (a line to add). Give about 3 lines of context \
above and below each change. When those lines are not unique in the file, put the \
line of the class or function before the hunk with `@@ <line>`; a bare `@@` starts a \
hunk without one. `*** End of File` after a hunk says that the hunk ends the file. \
Every line of an added file starts with `+`. Paths are relative to the user's working \
directory, start with ~ for the home directory, or are absolute. The patch changes all \
its files or none. A delete or a move always waits for the user's approval.";

/// Edits files with a patch: adds, deletes, updates and moves them, all or nothing.
///
/// It declares every path of the patch for writing, the target of a move too, and
/// marks a delete or a move as destructive, which the permission engine asks about in
/// every mode. Before it writes a file it records the original in the context's
/// journal, and it writes nothing when the journal fails.
#[derive(Debug, Clone, Copy)]
pub struct ApplyPatchTool {
    engine: Engine,
}

/// The two entry points of the patch engine. Tests put in a stand-in while the engine
/// of `efr-patch` is not built.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Engine {
    pub(crate) parse: fn(&str) -> Result<Patch, PatchError>,
    pub(crate) apply: fn(&Patch, &dyn Files) -> Result<Vec<FileChange>, PatchError>,
}

impl Engine {
    /// The engine of `efr-patch`.
    const PATCH: Engine = Engine { parse: efr_patch::parse, apply: efr_patch::apply };
}

impl ApplyPatchTool {
    /// The tool's name.
    pub const NAME: &'static str = "apply_patch";

    /// The tool.
    pub fn new() -> Self {
        ApplyPatchTool { engine: Engine::PATCH }
    }

    /// The tool with another engine, for tests.
    #[cfg(test)]
    pub(crate) fn with_engine(engine: Engine) -> Self {
        ApplyPatchTool { engine }
    }

    /// The call's patch with absolute paths, or why it is not one.
    fn plan(&self, ctx: &ToolContext, input: &Value) -> Result<Planned, ToolError> {
        let text = freeform_text(input).ok_or_else(|| ToolError::InvalidInput {
            tool: Self::NAME.to_owned(),
            source: serde_json::Error::custom("the input must be the text of a patch"),
        })?;
        let written = (self.engine.parse)(text).map_err(|source| ToolError::Patch { source })?;
        let mut failed = None;
        let patch = written.clone().map_paths(|path| {
            resolve(ctx, &path.to_string_lossy()).unwrap_or_else(|error| {
                failed.get_or_insert(error);
                PathBuf::new()
            })
        });
        if let Some(error) = failed {
            return Err(error);
        }
        let mut written_as: HashMap<PathBuf, String> = HashMap::new();
        for (resolved, raw) in patch.operations.iter().zip(&written.operations) {
            let pairs =
                [(Some(resolved.path()), Some(raw.path())), (resolved.move_to(), raw.move_to())];
            for (resolved, raw) in pairs {
                if let (Some(resolved), Some(raw)) = (resolved, raw) {
                    written_as
                        .entry(resolved.to_path_buf())
                        .or_insert_with(|| raw.display().to_string());
                }
            }
        }
        Ok(Planned { patch, written_as })
    }
}

impl Default for ApplyPatchTool {
    fn default() -> Self {
        ApplyPatchTool::new()
    }
}

/// A call's patch, ready to apply.
struct Planned {
    /// The patch with every path absolute.
    patch: Patch,
    /// Each absolute path as the patch wrote it, for the result the model reads.
    written_as: HashMap<PathBuf, String>,
}

impl Planned {
    /// Every path of the patch, each once, in the order the patch names them.
    fn paths(&self) -> Vec<PathBuf> {
        self.patch.paths().into_iter().map(Path::to_path_buf).collect()
    }

    /// `path` as the patch wrote it.
    fn shown(&self, path: &Path) -> String {
        self.written_as.get(path).cloned().unwrap_or_else(|| path.display().to_string())
    }
}

/// What the files of a patch were before the call.
#[derive(Debug, Default)]
struct Originals {
    /// The snapshot of each path, for the journal and to put a file back.
    snapshots: HashMap<PathBuf, FileSnapshot>,
    /// The text of each path that holds a file, for the engine.
    texts: HashMap<PathBuf, String>,
}

impl Originals {
    fn snapshot(&self, path: &Path) -> FileSnapshot {
        self.snapshots
            .get(path)
            .cloned()
            .unwrap_or_else(|| FileSnapshot::new(path, Original::Missing))
    }

    fn text(&self, path: &Path) -> Option<&str> {
        self.texts.get(path).map(String::as_str)
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::freeform(
            Self::NAME,
            DESCRIPTION,
            ToolGrammar::Lark(efr_patch::GRAMMAR.to_owned()),
        )
    }

    fn requirements(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        let planned = self.plan(ctx, input)?;
        let destructive = planned.patch.operations.iter().any(Operation::is_destructive);
        let requirements = planned
            .paths()
            .into_iter()
            .fold(ToolRequirements::none(), ToolRequirements::with_write);
        Ok(requirements.with_destructive(destructive))
    }

    /// The diff of every file of the patch, each within the bounds of a written diff,
    /// with a line `delete <path>` or `move <from> -> <to>` before a delete and a move.
    /// Nothing for a patch that does not apply; its call fails when it runs.
    async fn preview(&self, ctx: &ToolContext, input: &Value) -> Option<String> {
        let planned = self.plan(ctx, input).ok()?;
        let home = ctx.home.clone();
        let paths = planned.paths();
        let originals = blocking(&ctx.cwd, move || read_originals(&home, &paths)).await.ok()?;
        let changes = (self.engine.apply)(&planned.patch, &originals.texts).ok()?;
        Some(preview(&changes, &originals))
    }

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        _out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let planned = self.plan(&ctx, &input)?;
        let home = ctx.home.clone();
        let paths = planned.paths();
        let originals = blocking(&ctx.cwd, move || read_originals(&home, &paths)).await?;
        let changes = match (self.engine.apply)(&planned.patch, &originals.texts) {
            Ok(changes) => changes,
            Err(error) => return Ok(ToolResult::error(failure(&error, &planned.patch))),
        };
        let written = written(&changes, &originals);

        // NOTE: the paths are put back in reverse order of their first change, so a
        // path that two changes touched gets its original, not a state in between.
        let mut touched: Vec<PathBuf> = Vec::new();
        for change in &changes {
            if let Err(error) = write_change(&ctx, change, &originals, &mut touched).await {
                let restore = touched.clone();
                let snapshots: Vec<FileSnapshot> =
                    restore.iter().rev().map(|path| originals.snapshot(path)).collect();
                let unrestored = blocking(&ctx.cwd, move || Ok(restore_all(&snapshots)))
                    .await
                    .unwrap_or(touched);
                return Ok(ToolResult::error(write_failure(&error, &unrestored)));
            }
        }
        Ok(ToolResult::ok(success(&changes, &planned)).with_written(written))
    }
}

/// Records the originals of `change` in the journal, then makes the change. Each path
/// goes into `touched` before anything is written there.
async fn write_change(
    ctx: &ToolContext,
    change: &FileChange,
    originals: &Originals,
    touched: &mut Vec<PathBuf>,
) -> Result<(), ToolError> {
    let mut paths = vec![change.path.clone()];
    if let ChangeKind::Moved { to, .. } = &change.kind {
        paths.push(to.clone());
    }
    for path in &paths {
        ctx.journal.record(JournalEntry::new(ctx.ids, originals.snapshot(path))).await?;
    }
    for path in paths {
        if !touched.contains(&path) {
            touched.push(path);
        }
    }
    let change = change.clone();
    let source = originals.snapshot(&change.path);
    blocking(&ctx.cwd, move || make_change(&change, &source)).await
}

/// Makes one change. A new content keeps the mode and the owner of `source`, the
/// file's original (of the source of a move), or gets mode 0644 for a new file.
fn make_change(change: &FileChange, source: &FileSnapshot) -> Result<(), ToolError> {
    let (mode, owner) = match &source.original {
        Original::File { mode, uid, gid, .. } => (*mode, Some((*uid, *gid))),
        _ => (NEW_FILE_MODE, None),
    };
    match &change.kind {
        ChangeKind::Added { content } | ChangeKind::Updated { content } => {
            write(&change.path, content.as_bytes(), mode, owner)
        }
        ChangeKind::Deleted => remove(&change.path),
        ChangeKind::Moved { to, content } => {
            write(to, content.as_bytes(), mode, owner)?;
            remove(&change.path)
        }
    }
}

/// Removes the file at `path`.
fn remove(path: &Path) -> Result<(), ToolError> {
    fs::remove_file(path).map_err(|source| ToolError::Remove { path: path.to_path_buf(), source })
}

/// Puts back each snapshot, in order, and returns the paths that could not be put
/// back.
fn restore_all(snapshots: &[FileSnapshot]) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for snapshot in snapshots {
        let restored = match &snapshot.original {
            Original::File { mode, uid, gid, content } => {
                write(&snapshot.path, content, *mode, Some((*uid, *gid)))
            }
            _ => match fs::remove_file(&snapshot.path) {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    Err(ToolError::Remove { path: snapshot.path.clone(), source: error })
                }
                _ => Ok(()),
            },
        };
        if let Err(error) = restored {
            tracing::warn!(path = %snapshot.path.display(), error = %efr_stdx::with_causes(&error), "apply_patch could not put a file back");
            failed.push(snapshot.path.clone());
        }
    }
    failed
}

/// Reads the original of each of `paths`, after the checks of the file tools.
fn read_originals(home: &Home, paths: &[PathBuf]) -> Result<Originals, ToolError> {
    let mut originals = Originals::default();
    for path in paths {
        let (snapshot, text) = original(home, path)?;
        if let Some(text) = text {
            originals.texts.insert(path.clone(), text);
        }
        originals.snapshots.insert(path.clone(), snapshot);
    }
    Ok(originals)
}

/// The original at `path` and its text: refused through a symbolic link, for anything
/// that is not a regular file, over 16 MiB, and for bytes that are not text.
fn original(home: &Home, path: &Path) -> Result<(FileSnapshot, Option<String>), ToolError> {
    check_real(home, path)?;
    let read_error = |source| ToolError::Read { path: path.to_path_buf(), source };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((FileSnapshot::new(path, Original::Missing), None));
        }
        Err(source) => return Err(read_error(source)),
    };
    // A dangling link passes the symlink check (nothing real is behind it), and is not
    // a file to edit either.
    if !metadata.file_type().is_file() {
        return Err(ToolError::NotAFile { path: path.to_path_buf() });
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ToolError::TooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            limit: MAX_FILE_BYTES,
        });
    }
    let mut content = Vec::new();
    // `take` keeps a file that grows after the size check within the limit.
    File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES).read_to_end(&mut content))
        .map_err(read_error)?;
    if content[..content.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Err(ToolError::NotText { path: path.to_path_buf() });
    }
    let text = String::from_utf8(content.clone())
        .map_err(|_| ToolError::NotText { path: path.to_path_buf() })?;
    let original = Original::File {
        mode: metadata.mode() & 0o7777,
        uid: metadata.uid(),
        gid: metadata.gid(),
        content,
    };
    Ok((FileSnapshot::new(path, original), Some(text)))
}

/// What each change does to its file, with its diff: the diffs of all files together
/// stay within [`MAX_CALL_DIFF_LINES`](efr_protocol::MAX_CALL_DIFF_LINES) lines, and
/// a file after that has its header and the number of lines left out.
fn written(changes: &[FileChange], originals: &Originals) -> Vec<WrittenFile> {
    let mut left = efr_protocol::MAX_CALL_DIFF_LINES;
    let mut files = Vec::with_capacity(changes.len());
    for change in changes {
        let (file, shown) = written_file(change, originals, left);
        left = left.saturating_sub(shown);
        files.push(file);
    }
    files
}

/// One change as a [`WrittenFile`], with at most `max_lines` lines of diff, and the
/// number of lines it shows.
fn written_file(
    change: &FileChange,
    originals: &Originals,
    max_lines: usize,
) -> (WrittenFile, usize) {
    let path = change.path.as_path();
    let old = originals.text(path).map(|text| (path, text));
    let (target, kind, diff) = match &change.kind {
        ChangeKind::Added { content } | ChangeKind::Updated { content } => {
            let kind = if old.is_some() { WrittenKind::Changed } else { WrittenKind::Created };
            (path, kind, change_diff(old, Some((path, content)), max_lines))
        }
        ChangeKind::Deleted => (path, WrittenKind::Deleted, change_diff(old, None, max_lines)),
        ChangeKind::Moved { to, content } => (
            to.as_path(),
            WrittenKind::Moved { from: path.to_path_buf() },
            change_diff(old, Some((to, content)), max_lines),
        ),
    };
    let shown = diff.as_ref().map_or(0, |(_, shown)| *shown);
    let file =
        WrittenFile { path: target.to_path_buf(), kind, binary: false, diff: diff.map(|(d, _)| d) };
    (file, shown)
}

/// The approval's preview of `changes`.
fn preview(changes: &[FileChange], originals: &Originals) -> String {
    let max_lines = efr_protocol::MAX_CALL_DIFF_LINES;
    let mut text = String::new();
    for (at, change) in changes.iter().enumerate() {
        if text.len() > MAX_PREVIEW_BYTES {
            text.push_str(&format!("[... {} more files of the patch]\n", changes.len() - at));
            break;
        }
        match &change.kind {
            ChangeKind::Deleted => {
                text.push_str(&format!("delete {}\n", change.path.display()));
            }
            ChangeKind::Moved { to, .. } => {
                text.push_str(&format!("move {} -> {}\n", change.path.display(), to.display()));
            }
            ChangeKind::Added { .. } | ChangeKind::Updated { .. } => {}
        }
        let (file, _) = written_file(change, originals, max_lines);
        match file.diff {
            Some(diff) => text.push_str(&diff.text),
            None => text.push_str(&format!("{}: {UNCHANGED}\n", change.path.display())),
        }
    }
    text
}

/// The short answer of a patch that applied, such as
/// `Success. Updated: a.rs, b.rs; Added: c.rs; Deleted: d.rs; Moved: e.rs -> f.rs`.
fn success(changes: &[FileChange], planned: &Planned) -> String {
    let mut updated = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let mut moved = Vec::new();
    for change in changes {
        let path = planned.shown(&change.path);
        match &change.kind {
            ChangeKind::Updated { .. } => updated.push(path),
            ChangeKind::Added { .. } => added.push(path),
            ChangeKind::Deleted => deleted.push(path),
            ChangeKind::Moved { to, .. } => {
                moved.push(format!("{path} -> {}", planned.shown(to)));
            }
        }
    }
    let groups: Vec<String> =
        [("Updated", updated), ("Added", added), ("Deleted", deleted), ("Moved", moved)]
            .into_iter()
            .filter(|(_, paths)| !paths.is_empty())
            .map(|(name, paths)| format!("{name}: {}", paths.join(", ")))
            .collect();
    if groups.is_empty() {
        return "Success. No file changed.".to_owned();
    }
    format!("Success. {}", groups.join("; "))
}

/// What the model reads when the patch does not apply: the error, for a hunk that
/// matches nowhere its `@@` lines and the nearest lines of the file, then that no file
/// changed.
fn failure(error: &PatchError, patch: &Patch) -> String {
    let mut lines = vec![format!("{error}.")];
    if let PatchError::NoMatch { path, hunk, nearest } = error {
        let anchors = patch
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::Update { path: updated, hunks, .. } if updated == path => {
                    hunks.get(hunk.saturating_sub(1))
                }
                _ => None,
            })
            .map(|hunk| hunk.anchors.clone())
            .unwrap_or_default();
        if !anchors.is_empty() {
            lines.push(format!("Its @@ lines: {}.", anchors.join(" | ")));
        }
        if nearest.is_empty() {
            lines.push("No line of the file is near its lines.".to_owned());
        } else {
            lines.push("The nearest lines of the file:".to_owned());
            lines.extend(nearest.iter().map(|line| format!("{:>6} | {}", line.number, line.text)));
        }
        lines.push("Read the file again and send a corrected patch.".to_owned());
    }
    lines.push(NOTHING_CHANGED.to_owned());
    lines.join("\n")
}

/// What the model reads when a write failed after the patch applied: the error, then
/// whether every file is as it was.
fn write_failure(error: &ToolError, unrestored: &[PathBuf]) -> String {
    let error = efr_stdx::with_causes(error);
    if unrestored.is_empty() {
        return format!("{error}. {NOTHING_CHANGED}");
    }
    let paths: Vec<String> = unrestored.iter().map(|path| path.display().to_string()).collect();
    format!(
        "{error}. efr could not put these files back as they were: {}. Tell the user to check them.",
        paths.join(", ")
    )
}

#[cfg(test)]
mod tests;
