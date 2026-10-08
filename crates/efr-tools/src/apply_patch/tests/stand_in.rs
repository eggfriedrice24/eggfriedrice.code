//! A small stand-in for the patch engine, for the tests of the tool while the engine
//! of `efr-patch` is not built: a strict parser of the format and an exact match of
//! each hunk. It knows only what the tests' patches use. Once `efr_patch::parse`
//! works, the tests run on the real engine instead (see `engine` in `tests.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use efr_patch::{
    ChangeKind, FileChange, Files, Hunk, HunkLine, Operation, ParseProblem, Patch, PatchError,
};

use crate::apply_patch::Engine;

/// The stand-in as an engine.
pub(super) const ENGINE: Engine = Engine { parse, apply };

fn parse(text: &str) -> Result<Patch, PatchError> {
    let lines: Vec<&str> = text.lines().collect();
    let problem = |line: usize, problem| PatchError::Parse { line: line + 1, problem };
    if lines.first().map(|line| line.trim()) != Some("*** Begin Patch") {
        return Err(problem(0, ParseProblem::NoBegin));
    }
    let end = lines.len() - 1;
    if lines[end].trim() != "*** End Patch" {
        return Err(problem(end, ParseProblem::NoEnd));
    }
    let mut operations = Vec::new();
    let mut at = 1;
    while at < end {
        let line = lines[at];
        at += 1;
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut content = String::new();
            while at < end && lines[at].starts_with('+') {
                content.push_str(&lines[at][1..]);
                content.push('\n');
                at += 1;
            }
            operations.push(Operation::Add { path: path.into(), content });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(Operation::Delete { path: path.into() });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let move_to = lines[at].strip_prefix("*** Move to: ").map(PathBuf::from);
            if move_to.is_some() {
                at += 1;
            }
            let mut hunks: Vec<Hunk> = Vec::new();
            while at < end {
                let line = lines[at];
                if line == "*** End of File" {
                    if let Some(hunk) = hunks.last_mut() {
                        hunk.end_of_file = true;
                    }
                } else if line.starts_with("*** ") {
                    break;
                } else if let Some(anchor) = line.strip_prefix("@@") {
                    if hunks.last().is_none_or(|hunk| !hunk.lines.is_empty()) {
                        hunks.push(Hunk::default());
                    }
                    let anchor = anchor.trim();
                    if !anchor.is_empty() {
                        hunks.last_mut().unwrap().anchors.push(anchor.to_owned());
                    }
                } else {
                    if hunks.is_empty() {
                        hunks.push(Hunk::default());
                    }
                    let hunk_line = match line.split_at_checked(1) {
                        Some((" ", text)) => HunkLine::Context(text.to_owned()),
                        Some(("-", text)) => HunkLine::Remove(text.to_owned()),
                        Some(("+", text)) => HunkLine::Add(text.to_owned()),
                        None => HunkLine::Context(String::new()),
                        Some(_) => return Err(problem(at, ParseProblem::NotAHunkLine)),
                    };
                    hunks.last_mut().unwrap().lines.push(hunk_line);
                }
                at += 1;
            }
            operations.push(Operation::Update { path: path.into(), move_to, hunks });
        } else {
            return Err(problem(at - 1, ParseProblem::NotAnOperation));
        }
    }
    if operations.is_empty() {
        return Err(problem(end, ParseProblem::Empty));
    }
    Ok(Patch { operations })
}

fn apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError> {
    let mut changes: Vec<FileChange> = Vec::new();
    let mut current: HashMap<PathBuf, Option<String>> = HashMap::new();
    let text = |current: &HashMap<PathBuf, Option<String>>, path: &Path| {
        current.get(path).cloned().unwrap_or_else(|| files.text(path).map(str::to_owned))
    };
    for operation in &patch.operations {
        let change = match operation {
            Operation::Add { path, content } => {
                current.insert(path.clone(), Some(content.clone()));
                FileChange {
                    path: path.clone(),
                    kind: ChangeKind::Added { content: content.clone() },
                }
            }
            Operation::Delete { path } => {
                text(&current, path).ok_or_else(|| PatchError::Missing { path: path.clone() })?;
                current.insert(path.clone(), None);
                FileChange { path: path.clone(), kind: ChangeKind::Deleted }
            }
            Operation::Update { path, move_to, hunks } => {
                let old = text(&current, path)
                    .ok_or_else(|| PatchError::Missing { path: path.clone() })?;
                let content = apply_hunks(path, &old, hunks)?;
                match move_to {
                    Some(to) => {
                        current.insert(path.clone(), None);
                        current.insert(to.clone(), Some(content.clone()));
                        let kind = ChangeKind::Moved { to: to.clone(), content };
                        FileChange { path: path.clone(), kind }
                    }
                    None => {
                        current.insert(path.clone(), Some(content.clone()));
                        FileChange { path: path.clone(), kind: ChangeKind::Updated { content } }
                    }
                }
            }
        };
        match changes.iter_mut().find(|known| known.path == change.path) {
            Some(known) => *known = change,
            None => changes.push(change),
        }
    }
    Ok(changes)
}

fn apply_hunks(path: &Path, old: &str, hunks: &[Hunk]) -> Result<String, PatchError> {
    let mut lines: Vec<String> = old.lines().map(str::to_owned).collect();
    let mut cursor = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let want = hunk.old_lines();
        let found = (cursor..=lines.len().saturating_sub(want.len())).find(|&start| {
            lines.len() >= start + want.len()
                && lines[start..start + want.len()].iter().zip(&want).all(|(a, b)| a == b)
        });
        let Some(start) = found else {
            return Err(PatchError::NoMatch {
                path: path.to_path_buf(),
                hunk: index + 1,
                nearest: Vec::new(),
            });
        };
        let new: Vec<String> = hunk.new_lines().into_iter().map(str::to_owned).collect();
        cursor = start + new.len();
        lines.splice(start..start + want.len(), new);
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(text)
}
