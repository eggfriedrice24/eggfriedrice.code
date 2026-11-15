//! The legacy mark of a model list: a model is legacy when the same list has a newer
//! model of its family.
//!
//! The family of an id is the id without its version and its date, and the version is
//! the first run of `-` parts that hold only digits and dots, each part at most 4 bytes:
//! `claude-opus-5-5` is the family `claude-opus-*` at version 5.5, `gpt-6.1-sol` the
//! family `gpt-*-sol` at version 6.1. A date ends the version and is no part of the
//! family: a part of more than 4 digits (`20251001`), a year from 1990 to 2100 that one
//! or two parts of 2 digits follow (`2025-08-07`), or 4 digits with a leading zero
//! (`0613`, month and day). So `gpt-5-2025-08-07` is `gpt-*` at version 5, the same
//! version as `gpt-5`. An id with no version, such as `o3` or `gpt-4o`, is never legacy.
//! The versions compare number by number, so 6.1 is newer than 6, and 6 is newer than
//! 5.6.
//!
//! [`KEEP`] lists the models that stay out of the rule, also with a date: the user keeps
//! them with the current models although a newer one of their family exists. The
//! default model of the list is never legacy, so the phone never hides the model in use.
//!
//! A known limit: the family is the text around the version, so an old name order never
//! meets a new one. `claude-3-7-sonnet-20250219` is `claude-*-sonnet`, and
//! `claude-sonnet-4-5` is `claude-sonnet-*`, so the first stays current next to the
//! second.

use efr_protocol::ModelInfo;

/// The models that are never legacy, by the id without its date.
const KEEP: &[&str] = &["claude-fable-5-1"];

/// The longest part of a version, in bytes. A longer run of digits is a date.
const VERSION_PART_MAX: usize = 4;

/// The years that can start a date of an id.
const DATE_YEARS: std::ops::RangeInclusive<u32> = 1990..=2100;

/// Sets [`ModelInfo::legacy`] for each model of `models`, by the ids of the same list.
pub(crate) fn mark(models: &mut [ModelInfo]) {
    let versions: Vec<Option<(String, Vec<u32>)>> =
        models.iter().map(|model| family(&model.id)).collect();
    for (at, model) in models.iter_mut().enumerate() {
        model.legacy = !model.default
            && !KEEP.contains(&undated(&model.id).as_str())
            && versions[at].as_ref().is_some_and(|(own, version)| {
                versions.iter().flatten().any(|(other, newer)| other == own && newer > version)
            });
    }
}

/// The family and the version of `id`, or `None` when it has no version.
fn family(id: &str) -> Option<(String, Vec<u32>)> {
    let parts: Vec<&str> = id.split('-').collect();
    let starts = |at: usize| is_version(parts[at]) && date_len(&parts, at) == 0;
    let start = (0..parts.len()).find(|&at| starts(at))?;
    let end = (start..parts.len()).find(|&at| !starts(at)).unwrap_or(parts.len());
    let mut version = Vec::new();
    for part in &parts[start..end] {
        for number in part.split('.') {
            version.push(number.parse().ok()?);
        }
    }
    let mut name: Vec<&str> = parts[..start].to_vec();
    name.push("*");
    name.extend(without_dates(&parts[end..]));
    Some((name.join("-"), version))
}

/// `id` without the parts of its date, such as `claude-fable-5-1` for
/// `claude-fable-5-1-20260301`.
fn undated(id: &str) -> String {
    let parts: Vec<&str> = id.split('-').collect();
    without_dates(&parts).join("-")
}

/// The parts of `parts` that are no part of a date.
fn without_dates<'a>(parts: &[&'a str]) -> Vec<&'a str> {
    let mut kept = Vec::new();
    let mut at = 0;
    while at < parts.len() {
        match date_len(parts, at) {
            0 => {
                kept.push(parts[at]);
                at += 1;
            }
            len => at += len,
        }
    }
    kept
}

/// The number of parts of the date that starts at `parts[at]`, or 0 when no date starts
/// there.
fn date_len(parts: &[&str], at: usize) -> usize {
    let part = parts[at];
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return 0;
    }
    if part.len() > VERSION_PART_MAX {
        return 1;
    }
    if part.len() != 4 {
        return 0;
    }
    if part.starts_with('0') {
        return 1;
    }
    let year = part.parse().is_ok_and(|year| DATE_YEARS.contains(&year));
    let two = |at: usize| {
        parts
            .get(at)
            .is_some_and(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()))
    };
    match (year, two(at + 1), two(at + 2)) {
        (true, true, true) => 3,
        (true, true, false) => 2,
        _ => 0,
    }
}

/// True for a part of a version, such as `5` or `6.1`.
fn is_version(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= VERSION_PART_MAX
        && part.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
        && !part.starts_with('.')
        && !part.ends_with('.')
        && !part.contains("..")
}

#[cfg(test)]
mod tests;
