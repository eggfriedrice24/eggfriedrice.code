//! The groups of the changed files of a turn, for the line at the end of a turn and
//! the table of `efr diff --stat`: the groups of `[diff.groups]` first, in the order of
//! the file, then the built-in groups from the path alone. The first that matches wins:
//!
//! - `generated`: a lock file (`Cargo.lock`, `package-lock.json`, `yarn.lock`, ...),
//!   a `*.snap` snapshot, or any file below a `fixtures/` directory;
//! - `tests`: a file below `tests/`, `test/`, `__tests__/` or `spec/`, a `tests.rs`,
//!   or a name such as `a_test.go`, `a.test.ts`, `a_spec.rb` or `a.spec.ts`;
//! - `docs`: a `*.md` file, or any file below `docs/`;
//! - `code`: every other file.
//!
//! Pure: no IO, so the tests are a table.

use efr_config::DiffGroups;
use efr_protocol::FileChange;

/// The group of every file that no other group takes.
pub(crate) const CODE: &str = "code";

/// Lock files, which a package manager writes.
const LOCK_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "go.sum",
    "Gemfile.lock",
    "composer.lock",
    "flake.lock",
    "mix.lock",
    "pubspec.lock",
    "Podfile.lock",
];

/// The directories whose files are tests.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec"];

/// The parts of a file name that mark a test, after a first character: `a_test.go`.
const TEST_MARKS: &[&str] = &["_test.", ".test.", "_spec.", ".spec."];

/// The group of the file at `path`, a path as `FileChange::path` gives it: the first
/// group of `groups` that matches, else a built-in group.
pub(crate) fn group_of<'a>(path: &str, groups: &'a DiffGroups) -> &'a str {
    groups.group_of(path).unwrap_or_else(|| built_in(path))
}

/// The built-in group of the file at `path`.
fn built_in(path: &str) -> &'static str {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    let below = |dirs: &[&str]| parts.iter().any(|part| dirs.contains(part));
    if LOCK_FILES.contains(&name) || name.ends_with(".snap") || below(&["fixtures"]) {
        return "generated";
    }
    let marked = TEST_MARKS.iter().any(|mark| name.find(mark).is_some_and(|at| at > 0));
    if below(TEST_DIRS) || name == "tests.rs" || marked {
        return "tests";
    }
    if name.ends_with(".md") || below(&["docs"]) {
        return "docs";
    }
    CODE
}

/// The files and lines of one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupCount {
    /// The group's name.
    pub(crate) name: String,
    /// The changed files of the group.
    pub(crate) files: usize,
    /// Lines added.
    pub(crate) added: u32,
    /// Lines removed.
    pub(crate) removed: u32,
}

impl GroupCount {
    /// The changed lines, added and removed.
    pub(crate) fn lines(&self) -> u64 {
        u64::from(self.added) + u64::from(self.removed)
    }
}

/// The groups of `files`, the largest first by changed lines; groups of equal size keep
/// the order of their first file.
pub(crate) fn breakdown(files: &[FileChange], groups: &DiffGroups) -> Vec<GroupCount> {
    let mut counts: Vec<GroupCount> = Vec::new();
    for file in files {
        let name = group_of(&file.path, groups);
        let at = match counts.iter().position(|count| count.name == name) {
            Some(at) => at,
            None => {
                let count = GroupCount { name: name.to_owned(), files: 0, added: 0, removed: 0 };
                counts.push(count);
                counts.len() - 1
            }
        };
        let count = &mut counts[at];
        count.files += 1;
        count.added = count.added.saturating_add(file.added);
        count.removed = count.removed.saturating_add(file.removed);
    }
    counts.sort_by_key(|count| std::cmp::Reverse(count.lines()));
    counts
}

/// True when every file of `counts` is code, or there is none.
pub(crate) fn only_code(counts: &[GroupCount]) -> bool {
    counts.iter().all(|count| count.name == CODE)
}

#[cfg(test)]
mod tests;
