//! The running jobs of a hidden shell, for the fresh context block after a compaction:
//! the shell's child processes, read from `/proc`. Between two tool calls nothing runs
//! in the foreground, so each child is a job that a call left in the background.

use std::fs;
use std::path::Path;

/// The most jobs that the block names.
const MAX_JOBS: usize = 10;

/// The longest command line of a job, in characters.
const MAX_LINE: usize = 200;

/// The command lines of the children of the process `pid`, from the `/proc` at `proc`;
/// `None` when the process or its list of children cannot be read.
pub(crate) fn of(proc: &Path, pid: u32) -> Option<Vec<String>> {
    let children = fs::read_to_string(proc.join(format!("{pid}/task/{pid}/children"))).ok()?;
    Some(
        children
            .split_whitespace()
            .filter_map(|child| command_line(&proc.join(child)))
            .take(MAX_JOBS)
            .collect(),
    )
}

/// The command line of the process whose `/proc` directory is `dir`, its arguments
/// joined by spaces and cut at [`MAX_LINE`] characters.
fn command_line(dir: &Path) -> Option<String> {
    let raw = fs::read(dir.join("cmdline")).ok()?;
    let words: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    if words.is_empty() {
        return None;
    }
    let line = words.join(" ");
    if line.chars().count() <= MAX_LINE {
        return Some(line);
    }
    let mut cut: String = line.chars().take(MAX_LINE).collect();
    cut.push_str("...");
    Some(cut)
}

#[cfg(test)]
mod tests;
