//! Patch text to a [`Patch`].

use std::ops::Range;
use std::path::PathBuf;

use crate::{Hunk, HunkLine, Operation, ParseProblem, Patch, PatchError};

const BEGIN: &str = "*** Begin Patch";
const END: &str = "*** End Patch";
const ADD: &str = "*** Add File:";
const DELETE: &str = "*** Delete File:";
const UPDATE: &str = "*** Update File:";
const MOVE: &str = "*** Move to:";
const END_OF_FILE: &str = "*** End of File";
const ANCHOR: &str = "@@";

/// Reads the text of an `apply_patch` call.
///
/// The text follows [`GRAMMAR`](crate::GRAMMAR). The reader is lenient where models
/// are known to slip, as Codex's is: whitespace around the marker lines, blank lines
/// around the patch, `\r\n` line ends, a missing final newline, and a patch wrapped in
/// a heredoc (`<<'EOF'` ... `EOF`). The README lists every tolerance. Anything else
/// that is not a patch is [`PatchError::Parse`] with the line and the problem.
pub fn parse(text: &str) -> Result<Patch, PatchError> {
    let lines: Vec<&str> =
        text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).collect();
    let body = body(&lines)?;
    let mut parser = Parser::default();
    for index in body.clone() {
        if parser.line(index + 1, lines[index])? == Step::End {
            return match lines[index + 1..body.end].iter().position(|line| !blank(line)) {
                Some(after) => Err(error(index + 2 + after, ParseProblem::AfterEnd)),
                None => Err(error(body.end + 1, ParseProblem::AfterEnd)),
            };
        }
    }
    parser.finish(body.end + 1)
}

fn error(line: usize, problem: ParseProblem) -> PatchError {
    PatchError::Parse { line, problem }
}

fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// The indexes of the lines between `*** Begin Patch` and `*** End Patch`, inside a
/// heredoc when the text is wrapped in one.
fn body(lines: &[&str]) -> Result<Range<usize>, PatchError> {
    let first = lines.iter().position(|line| !blank(line));
    let last = lines.iter().rposition(|line| !blank(line));
    let (Some(first), Some(last)) = (first, last) else {
        return Err(error(1, ParseProblem::NoBegin));
    };
    if lines[first].trim() != BEGIN && heredoc(lines[first], lines[last]) && first < last {
        return markers(lines, first + 1, last);
    }
    markers(lines, first, last + 1)
}

/// The body between the first and the last line of `lines[from..to]` that are not
/// blank, which must be the begin and the end marker.
fn markers(lines: &[&str], from: usize, to: usize) -> Result<Range<usize>, PatchError> {
    let inner = &lines[from..to];
    let first = inner.iter().position(|line| !blank(line)).map(|index| from + index);
    let last = inner.iter().rposition(|line| !blank(line)).map(|index| from + index);
    let (Some(first), Some(last)) = (first, last) else {
        return Err(error(from + 1, ParseProblem::NoBegin));
    };
    if lines[first].trim() != BEGIN {
        return Err(error(first + 1, ParseProblem::NoBegin));
    }
    if first == last || lines[last].trim() != END {
        return Err(error(last + 1, ParseProblem::NoEnd));
    }
    Ok(first + 1..last)
}

/// True for the lines of a heredoc that wraps a patch, the mistake Codex accepts:
/// `<<EOF`, `<<'EOF'` or `<<"EOF"` first, and a last line that ends with `EOF`.
fn heredoc(first: &str, last: &str) -> bool {
    matches!(first.trim(), "<<EOF" | "<<'EOF'" | "<<\"EOF\"") && last.trim().ends_with("EOF")
}

/// What a line did to the parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Next,
    /// The line is `*** End Patch`; only blank lines may follow it.
    End,
}

/// What the lines read so far belong to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    /// Between operations: after `*** Begin Patch` or a delete.
    #[default]
    Between,
    /// The `+` lines of an added file.
    Add,
    /// The lines of an update. `update` is the line of its `*** Update File:`, and
    /// `hunk` the line where its last hunk starts.
    Update { update: usize, hunk: usize },
}

#[derive(Debug, Default)]
struct Parser {
    operations: Vec<Operation>,
    mode: Mode,
}

impl Parser {
    fn line(&mut self, number: usize, line: &str) -> Result<Step, PatchError> {
        match self.mode {
            Mode::Between | Mode::Add => self.outside_update(number, line),
            Mode::Update { update, .. } => self.inside_update(number, line, update),
        }
    }

    /// Between operations and in an added file, a marker line may have whitespace at
    /// both ends.
    fn outside_update(&mut self, number: usize, line: &str) -> Result<Step, PatchError> {
        let trimmed = line.trim();
        if let Some(step) = self.marker(number, trimmed)? {
            return Ok(step);
        }
        match self.mode {
            Mode::Add => {
                let (Some(added), Some(Operation::Add { content, .. })) =
                    (line.strip_prefix('+'), self.operations.last_mut())
                else {
                    return Err(error(number, ParseProblem::NotAnAddLine));
                };
                content.push_str(added);
                content.push('\n');
                Ok(Step::Next)
            }
            Mode::Between | Mode::Update { .. } if trimmed.is_empty() => Ok(Step::Next),
            Mode::Between | Mode::Update { .. } => Err(error(number, ParseProblem::NotAnOperation)),
        }
    }

    /// In an update, only trailing whitespace is ignored on a marker line: a line that
    /// starts with a space is a context line, even when its text looks like a marker.
    fn inside_update(
        &mut self,
        number: usize,
        line: &str,
        update: usize,
    ) -> Result<Step, PatchError> {
        let trimmed = line.trim_end();
        if let Some(step) = self.marker(number, trimmed)? {
            return Ok(step);
        }
        let Some(Operation::Update { move_to, hunks, .. }) = self.operations.last_mut() else {
            unreachable!("the mode is Update only while the last operation is an update");
        };
        if let Some(target) = trimmed.strip_prefix(MOVE) {
            if !hunks.is_empty() || move_to.is_some() {
                return Err(error(number, ParseProblem::MisplacedMove));
            }
            *move_to = Some(path(number, target)?);
            return Ok(Step::Next);
        }
        if let Some(anchor) = anchor(trimmed) {
            match hunks.last_mut() {
                // NOTE: `@@` lines in a row stack: each one narrows the search for the
                // next, such as `@@ impl Foo` and then `@@ fn bar`.
                Some(last) if last.lines.is_empty() => last.anchors.extend(anchor),
                _ => {
                    hunks.push(Hunk { anchors: anchor.into_iter().collect(), ..Hunk::default() });
                    self.mode = Mode::Update { update, hunk: number };
                }
            }
            return Ok(Step::Next);
        }
        let closed = hunks.last().is_some_and(|last| last.end_of_file);
        if trimmed == END_OF_FILE {
            return match hunks.last_mut() {
                Some(last) if !last.end_of_file && !last.lines.is_empty() => {
                    last.end_of_file = true;
                    Ok(Step::Next)
                }
                Some(_) if closed => Err(error(number, ParseProblem::AfterEndOfFile)),
                _ => Err(error(number, ParseProblem::EmptyHunk)),
            };
        }
        if closed {
            return if blank(line) {
                Ok(Step::Next)
            } else {
                Err(error(number, ParseProblem::AfterEndOfFile))
            };
        }
        let hunk_line = if line.is_empty() {
            // NOTE: a line with nothing on it is an empty context line, as in Codex:
            // models drop the space of an empty context line.
            HunkLine::Context(String::new())
        } else if let Some(text) = line.strip_prefix(' ') {
            HunkLine::Context(text.to_owned())
        } else if let Some(text) = line.strip_prefix('-') {
            HunkLine::Remove(text.to_owned())
        } else if let Some(text) = line.strip_prefix('+') {
            HunkLine::Add(text.to_owned())
        } else {
            return Err(error(number, ParseProblem::NotAHunkLine));
        };
        if hunks.is_empty() {
            hunks.push(Hunk::default());
            self.mode = Mode::Update { update, hunk: number };
        }
        if let Some(last) = hunks.last_mut() {
            last.lines.push(hunk_line);
        }
        Ok(Step::Next)
    }

    /// Handles a line that starts an operation or ends the patch, and closes the
    /// operation before it. `None` when the line is no such marker.
    fn marker(&mut self, number: usize, trimmed: &str) -> Result<Option<Step>, PatchError> {
        if trimmed == END {
            self.close()?;
            return Ok(Some(Step::End));
        }
        let operation = if let Some(rest) = trimmed.strip_prefix(ADD) {
            Operation::Add { path: path(number, rest)?, content: String::new() }
        } else if let Some(rest) = trimmed.strip_prefix(DELETE) {
            Operation::Delete { path: path(number, rest)? }
        } else if let Some(rest) = trimmed.strip_prefix(UPDATE) {
            Operation::Update { path: path(number, rest)?, move_to: None, hunks: Vec::new() }
        } else if trimmed.starts_with(MOVE) && !matches!(self.mode, Mode::Update { .. }) {
            return Err(error(number, ParseProblem::MisplacedMove));
        } else {
            return Ok(None);
        };
        self.close()?;
        self.mode = match operation {
            Operation::Add { .. } => Mode::Add,
            Operation::Delete { .. } => Mode::Between,
            Operation::Update { .. } => Mode::Update { update: number, hunk: number },
        };
        self.operations.push(operation);
        Ok(Some(Step::Next))
    }

    /// Checks the operation that ends here: an update needs a hunk or a move, and its
    /// last hunk needs a line.
    fn close(&mut self) -> Result<(), PatchError> {
        if let (Mode::Update { update, hunk }, Some(Operation::Update { move_to, hunks, .. })) =
            (self.mode, self.operations.last())
        {
            match hunks.last() {
                None if move_to.is_none() => {
                    return Err(error(update, ParseProblem::EmptyUpdate));
                }
                Some(last) if last.lines.is_empty() => {
                    return Err(error(hunk, ParseProblem::EmptyHunk));
                }
                _ => {}
            }
        }
        self.mode = Mode::Between;
        Ok(())
    }

    fn finish(mut self, end: usize) -> Result<Patch, PatchError> {
        self.close()?;
        if self.operations.is_empty() {
            return Err(error(end, ParseProblem::Empty));
        }
        Ok(Patch { operations: self.operations })
    }
}

/// The path after a marker. Whitespace around it is not part of it.
fn path(number: usize, rest: &str) -> Result<PathBuf, PatchError> {
    let path = rest.trim();
    if path.is_empty() {
        return Err(error(number, ParseProblem::NoPath));
    }
    Ok(PathBuf::from(path))
}

/// For an `@@` line, its anchor text: `Some(None)` for a bare `@@`. A unified diff
/// header such as `@@ -12,7 +12,8 @@ fn run()` is read as its trailing text, here
/// `fn run()`, because models trained on `git diff` write it.
fn anchor(trimmed: &str) -> Option<Option<String>> {
    if trimmed == ANCHOR {
        return Some(None);
    }
    let text = trimmed.strip_prefix("@@ ")?;
    if let Some(after) = diff_range(text) {
        let after = after.trim();
        return Some((!after.is_empty()).then(|| after.to_owned()));
    }
    Some(Some(text.to_owned()))
}

/// For `-12,7 +12,8 @@ rest`, `Some(" rest")`.
fn diff_range(text: &str) -> Option<&str> {
    fn range(text: &str, sign: char) -> Option<&str> {
        let text = text.strip_prefix(sign)?;
        let digits = text.find(|c: char| !c.is_ascii_digit() && c != ',').unwrap_or(text.len());
        let (numbers, rest) = text.split_at(digits);
        let valid = !numbers.is_empty()
            && numbers.split(',').count() <= 2
            && numbers.split(',').all(|part| !part.is_empty());
        valid.then_some(rest)
    }
    let rest = range(text, '-')?.strip_prefix(' ')?;
    range(rest, '+')?.trim_start().strip_prefix(ANCHOR)
}

#[cfg(test)]
mod tests;
