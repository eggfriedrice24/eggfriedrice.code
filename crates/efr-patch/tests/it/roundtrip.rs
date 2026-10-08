//! A patch made from a diff of two texts turns the first text into the second.

use std::collections::BTreeMap;
use std::path::PathBuf;

use efr_patch::{ChangeKind, FileChange, PatchError, apply, parse};
use proptest::prelude::*;

/// One step of a diff from the old lines to the new lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Same(usize),
    Gone(usize),
    New(usize),
}

/// A shortest edit script by the longest common subsequence. The texts are small.
fn diff(old: &[String], new: &[String]) -> Vec<Step> {
    let mut common = vec![vec![0_usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut steps) = (0, 0, Vec::new());
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            steps.push(Step::Same(i));
            i += 1;
            j += 1;
        } else if j < new.len() && (i == old.len() || common[i][j + 1] >= common[i + 1][j]) {
            steps.push(Step::New(j));
            j += 1;
        } else {
            steps.push(Step::Gone(i));
            i += 1;
        }
    }
    steps
}

/// The patch text of the diff, with `context` lines of context around each change.
/// A hunk that would have no old line takes the line before it (or after it) as
/// context, because a hunk of only `+` lines appends to the file. A hunk that ends
/// the file gets `*** End of File` when `mark_end` is set.
fn patch(old: &[String], new: &[String], steps: &[Step], context: usize, mark_end: bool) -> String {
    let changed: Vec<usize> =
        (0..steps.len()).filter(|&at| !matches!(steps[at], Step::Same(_))).collect();
    let has_old =
        |from: usize, to: usize| steps[from..to].iter().any(|step| !matches!(step, Step::New(_)));
    // NOTE: the ranges are merged first, so a range without an old line is one run of
    // `+` lines between two kept lines, and one kept line around it is enough.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &at in &changed {
        let from = at.saturating_sub(context);
        let to = (at + context + 1).min(steps.len());
        match ranges.last_mut() {
            Some(last) if from <= last.1 => last.1 = to,
            _ => ranges.push((from, to)),
        }
    }
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (mut from, mut to) in ranges {
        if !has_old(from, to) {
            if from > 0 {
                from -= 1;
            } else if to < steps.len() {
                to += 1;
            }
        }
        match merged.last_mut() {
            Some(last) if from < last.1 => last.1 = last.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    let mut text = String::from("*** Begin Patch\n*** Update File: f\n");
    for (from, to) in merged {
        text.push_str("@@\n");
        for step in &steps[from..to] {
            let (marker, line) = match *step {
                Step::Same(i) => (' ', &old[i]),
                Step::Gone(i) => ('-', &old[i]),
                Step::New(j) => ('+', &new[j]),
            };
            text.push(marker);
            text.push_str(line);
            text.push('\n');
        }
        let ends_file = steps[to..].iter().all(|step| matches!(step, Step::New(_)))
            && steps[from..to].iter().any(|step| !matches!(step, Step::New(_)))
            && to == steps.len();
        if mark_end && ends_file {
            text.push_str("*** End of File\n");
        }
    }
    text.push_str("*** End Patch\n");
    text
}

fn join(lines: &[String], crlf: bool, final_newline: bool) -> String {
    let ending = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(ending);
    if !lines.is_empty() && final_newline {
        text.push_str(ending);
    }
    text
}

/// Runs the patch of the diff from `old` to `new` and returns the new text, or the
/// error.
fn roundtrip(
    old: &[String],
    new: &[String],
    crlf: bool,
    final_newline: bool,
    context: usize,
    mark_end: bool,
) -> Result<(String, String), PatchError> {
    // NOTE: the engine keeps the old file's final newline; an empty file counts as
    // ending in one. Without a final newline, an empty last line is no line at all, so
    // such a text ends in a newline here. A new line takes the old file's first line
    // ending, so a file with no line ending at all gets `\n`.
    let final_newline = final_newline || old.last().is_none_or(String::is_empty);
    let crlf = crlf && (old.len() > 1 || (old.len() == 1 && final_newline));
    let before = join(old, crlf, final_newline);
    let expected = join(new, crlf, final_newline);
    let text = patch(old, new, &diff(old, new), context, mark_end);
    let patch = parse(&text).unwrap_or_else(|error| panic!("{error}:\n{text}"));
    let files: BTreeMap<PathBuf, String> = [(PathBuf::from("f"), before.clone())].into();
    let changes = apply(&patch, &files)?;
    let after = match changes.as_slice() {
        [] => before,
        [FileChange { kind: ChangeKind::Updated { content }, .. }] => content.clone(),
        other => panic!("expected one update, got {other:?}"),
    };
    Ok((after, expected))
}

fn body() -> impl Strategy<Value = String> {
    "[ab \t\u{e9}\u{2013}\u{201c}]{0,6}"
}

/// Old lines that are all different, even with whitespace and punctuation ignored,
/// and new lines that reuse some of them.
fn distinct_texts() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    (
        prop::collection::vec(body(), 0..20),
        prop::collection::vec((any::<bool>(), any::<usize>(), body()), 0..24),
    )
        .prop_map(|(bodies, picks)| {
            let old: Vec<String> =
                bodies.iter().enumerate().map(|(at, body)| format!("{body}#old{at}")).collect();
            let new = picks
                .into_iter()
                .enumerate()
                .map(|(at, (reuse, index, body))| match old.len() {
                    0 => format!("{body}#new{at}"),
                    len if reuse => old[index % len].clone(),
                    _ => format!("{body}#new{at}"),
                })
                .collect();
            (old, new)
        })
}

/// Texts from a few lines, so lines repeat and hunks can match in several places.
fn repeating_texts() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    let line = prop::sample::select(vec!["a", "b", "", "  a", "a  ", "c\u{2013}d", "c-d"])
        .prop_map(str::to_owned);
    (prop::collection::vec(line.clone(), 0..16), prop::collection::vec(line, 0..16))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn a_diff_of_distinct_lines_applies_to_the_second_text(
        (old, new) in distinct_texts(),
        crlf in any::<bool>(),
        final_newline in any::<bool>(),
        context in 0usize..4,
        mark_end in any::<bool>(),
    ) {
        prop_assume!(old != new);
        let (after, expected) = roundtrip(&old, &new, crlf, final_newline, context, mark_end)
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        prop_assert_eq!(after, expected);
    }

    #[test]
    fn a_diff_of_repeating_lines_applies_or_is_ambiguous(
        (old, new) in repeating_texts(),
        crlf in any::<bool>(),
        final_newline in any::<bool>(),
        context in 0usize..4,
        mark_end in any::<bool>(),
    ) {
        prop_assume!(old != new);
        match roundtrip(&old, &new, crlf, final_newline, context, mark_end) {
            Ok((after, expected)) => prop_assert_eq!(after, expected),
            Err(PatchError::Ambiguous { .. }) => {}
            Err(error) => prop_assert!(false, "{error}"),
        }
    }
}
